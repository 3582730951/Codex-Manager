use super::http_bridge::{DeliveryState, UpstreamCompletionState};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_TRACE_QUEUE_CAPACITY: usize = 2048;
const TRACE_FLUSH_WAIT_TIMEOUT_MS: u64 = 200;
const ENV_TRACE_QUEUE_CAPACITY: &str = "CODEXMANAGER_TRACE_QUEUE_CAPACITY";
const TRACE_PENDING_LINE_LIMIT: usize = 32;

static TRACE_WRITER: OnceLock<TraceAsyncWriter> = OnceLock::new();
static TRACE_SEQ: AtomicU64 = AtomicU64::new(1);
static TRACE_ERROR_TRACES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static TRACE_PENDING_LINES: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();

enum TraceCommand {
    Append {
        line: String,
        flush: bool,
        ack: Option<SyncSender<()>>,
    },
    ResetPath(PathBuf),
}

struct TraceAsyncWriter {
    tx: SyncSender<TraceCommand>,
    dropped: AtomicU64,
    queue_capacity: usize,
}

impl TraceAsyncWriter {
    fn new(path: PathBuf) -> Self {
        let queue_capacity = trace_queue_capacity();
        let (tx, rx) = mpsc::sync_channel::<TraceCommand>(queue_capacity);
        let _ = thread::Builder::new()
            .name("gateway-trace-writer".to_string())
            .spawn(move || trace_writer_loop(rx, TraceFileWriter::new(path)));
        Self {
            tx,
            dropped: AtomicU64::new(0),
            queue_capacity,
        }
    }

    fn append_line(&self, line: String, flush: bool) {
        if flush {
            let (ack_tx, ack_rx) = mpsc::sync_channel(0);
            if self
                .tx
                .send(TraceCommand::Append {
                    line,
                    flush: true,
                    ack: Some(ack_tx),
                })
                .is_err()
            {
                log::warn!("gateway trace enqueue failed: writer channel closed");
                return;
            }
            let _ = ack_rx.recv_timeout(Duration::from_millis(TRACE_FLUSH_WAIT_TIMEOUT_MS));
            return;
        }

        match self.tx.try_send(TraceCommand::Append {
            line,
            flush: false,
            ack: None,
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                if dropped == 1 || dropped % 1024 == 0 {
                    log::warn!(
                        "gateway trace queue full; dropped_lines={}, capacity={}",
                        dropped,
                        self.queue_capacity
                    );
                }
            }
            Err(TrySendError::Disconnected(_)) => {
                log::warn!("gateway trace enqueue failed: writer channel closed");
            }
        }
    }

    fn reset_path(&self, path: PathBuf) {
        if self.tx.send(TraceCommand::ResetPath(path)).is_err() {
            log::warn!("gateway trace reset-path failed: writer channel closed");
        }
    }
}

struct TraceFileWriter {
    path: PathBuf,
    writer: Option<BufWriter<File>>,
}

impl TraceFileWriter {
    fn new(path: PathBuf) -> Self {
        Self { path, writer: None }
    }

    fn reset_path(&mut self, next_path: PathBuf) {
        if self.path == next_path {
            return;
        }
        self.path = next_path;
        self.writer = None;
    }

    fn append_line(&mut self, line: &str, flush: bool) -> std::io::Result<()> {
        let writer = self.ensure_open_writer()?;
        writeln!(writer, "{line}")?;
        if flush {
            writer.flush()?;
        }
        Ok(())
    }

    fn ensure_open_writer(&mut self) -> std::io::Result<&mut BufWriter<File>> {
        if self.writer.is_none() {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            self.writer = Some(BufWriter::new(file));
        }
        Ok(self.writer.as_mut().expect("writer should be initialized"))
    }
}

fn trace_file_path_from_env() -> PathBuf {
    if let Ok(db_path) = std::env::var("CODEXMANAGER_DB_PATH") {
        let path = PathBuf::from(db_path);
        if let Some(parent) = path.parent() {
            return parent.join("gateway-trace.log");
        }
    }
    PathBuf::from("gateway-trace.log")
}

fn sanitize_text(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

fn short_fingerprint(value: &str) -> String {
    let mut hash: u64 = 14695981039346656037;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("{hash:016x}")
}

fn append_trace_line(line: String, flush: bool) {
    trace_writer().append_line(line, flush);
}

fn trace_error_traces() -> &'static Mutex<HashSet<String>> {
    TRACE_ERROR_TRACES.get_or_init(|| Mutex::new(HashSet::new()))
}

fn trace_pending_lines() -> &'static Mutex<HashMap<String, Vec<String>>> {
    TRACE_PENDING_LINES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn mark_trace_has_error(trace_id: &str) {
    if let Ok(mut traces) = trace_error_traces().lock() {
        traces.insert(trace_id.to_string());
    }
}

fn clear_trace_error(trace_id: &str) {
    if let Ok(mut traces) = trace_error_traces().lock() {
        traces.remove(trace_id);
    }
}

fn current_trace_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

fn buffer_trace_line(trace_id: &str, line: String) {
    if trace_id.trim().is_empty() {
        return;
    }
    let Ok(mut pending) = trace_pending_lines().lock() else {
        return;
    };
    let entry = pending.entry(trace_id.to_string()).or_default();
    if entry.len() < TRACE_PENDING_LINE_LIMIT {
        entry.push(line);
        return;
    }
    if entry
        .last()
        .is_some_and(|value| value.contains("event=TRACE_BUFFER_TRUNCATED"))
    {
        return;
    }
    let truncated_marker = format!(
        "ts={} event=TRACE_BUFFER_TRUNCATED trace_id={} dropped_after={}",
        current_trace_ts(),
        sanitize_text(trace_id),
        TRACE_PENDING_LINE_LIMIT,
    );
    if let Some(last) = entry.last_mut() {
        *last = truncated_marker;
    }
}

fn flush_trace_lines(trace_id: &str) {
    let lines = trace_pending_lines()
        .lock()
        .ok()
        .and_then(|mut pending| pending.remove(trace_id));
    if let Some(lines) = lines {
        for line in lines {
            append_trace_line(line, false);
        }
    }
}

fn clear_trace_state(trace_id: &str) {
    if let Ok(mut pending) = trace_pending_lines().lock() {
        pending.remove(trace_id);
    }
    clear_trace_error(trace_id);
}

#[cfg(test)]
fn trace_has_error(trace_id: &str) -> bool {
    trace_error_traces()
        .lock()
        .map(|traces| traces.contains(trace_id))
        .unwrap_or(false)
}

fn has_error_text(error: Option<&str>) -> bool {
    error
        .map(str::trim)
        .is_some_and(|value| !value.is_empty() && value != "-")
}

fn trace_writer() -> &'static TraceAsyncWriter {
    TRACE_WRITER.get_or_init(|| TraceAsyncWriter::new(trace_file_path_from_env()))
}

fn trace_writer_loop(rx: Receiver<TraceCommand>, mut writer: TraceFileWriter) {
    while let Ok(command) = rx.recv() {
        match command {
            TraceCommand::Append { line, flush, ack } => {
                if let Err(err) = writer.append_line(&line, flush) {
                    log::warn!(
                        "gateway trace write failed: path={}, err={}",
                        writer.path.display(),
                        err
                    );
                    writer.writer = None;
                }
                if let Some(ack) = ack {
                    let _ = ack.send(());
                }
            }
            TraceCommand::ResetPath(path) => writer.reset_path(path),
        }
    }
}

fn trace_queue_capacity() -> usize {
    std::env::var(ENV_TRACE_QUEUE_CAPACITY)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_TRACE_QUEUE_CAPACITY)
}

pub(super) fn reload_from_env() {
    let path = trace_file_path_from_env();
    trace_writer().reset_path(path);
}

pub(crate) fn next_trace_id() -> String {
    trace_writer().reset_path(trace_file_path_from_env());
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_millis())
        .unwrap_or(0);
    let seq = TRACE_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("trc_{millis}_{seq:x}")
}

pub(crate) fn log_request_start(
    trace_id: &str,
    key_id: &str,
    method: &str,
    path: &str,
    model: Option<&str>,
    reasoning: Option<&str>,
    is_stream: bool,
    protocol_type: &str,
) {
    let line = format!(
        "ts={} event=REQUEST_START trace_id={} key_id={} method={} path={} model={} reasoning={} stream={} protocol={}",
        current_trace_ts(),
        sanitize_text(trace_id),
        sanitize_text(key_id),
        sanitize_text(method),
        sanitize_text(path),
        sanitize_text(model.unwrap_or("-")),
        sanitize_text(reasoning.unwrap_or("-")),
        if is_stream { "true" } else { "false" },
        sanitize_text(protocol_type),
    );
    buffer_trace_line(trace_id, line);
}

pub(crate) fn log_request_body_preview(trace_id: &str, body: &[u8]) {
    let _ = (trace_id, body, super::trace_body_preview_max_bytes());
}

pub(crate) fn log_request_gate_wait(trace_id: &str, key_id: &str, path: &str, model: Option<&str>) {
    let _ = (trace_id, key_id, path, model);
}

pub(crate) fn log_request_gate_acquired(
    trace_id: &str,
    key_id: &str,
    path: &str,
    model: Option<&str>,
    wait_ms: u128,
) {
    let _ = (trace_id, key_id, path, model, wait_ms);
}

pub(crate) fn log_request_gate_skip(trace_id: &str, reason: &str) {
    let _ = (trace_id, reason);
}

pub(crate) fn log_candidate_start(
    trace_id: &str,
    idx: usize,
    total: usize,
    account_id: &str,
    strip_session_affinity: bool,
) {
    let line = format!(
        "ts={} event=CANDIDATE_START trace_id={} candidate={}/{} account_id={} strip_session_affinity={}",
        current_trace_ts(),
        sanitize_text(trace_id),
        idx + 1,
        total,
        sanitize_text(account_id),
        if strip_session_affinity { "true" } else { "false" },
    );
    buffer_trace_line(trace_id, line);
}

pub(crate) fn log_candidate_pool(
    trace_id: &str,
    key_id: &str,
    strategy: &str,
    rotation_source: &str,
    strategy_applied: bool,
    candidates: &[String],
) {
    let candidates = if candidates.is_empty() {
        "-".to_string()
    } else {
        candidates
            .iter()
            .map(|value| sanitize_text(value))
            .collect::<Vec<_>>()
            .join(",")
    };
    let line = format!(
        "ts={} event=CANDIDATE_POOL trace_id={} key_id={} strategy={} rotation_source={} strategy_applied={} candidates={}",
        current_trace_ts(),
        sanitize_text(trace_id),
        sanitize_text(key_id),
        sanitize_text(strategy),
        sanitize_text(rotation_source),
        if strategy_applied { "true" } else { "false" },
        candidates,
    );
    buffer_trace_line(trace_id, line);
}

pub(crate) fn log_candidate_skip(
    trace_id: &str,
    idx: usize,
    total: usize,
    account_id: &str,
    reason: &str,
) {
    let line = format!(
        "ts={} event=CANDIDATE_SKIP trace_id={} candidate={}/{} account_id={} reason={}",
        current_trace_ts(),
        sanitize_text(trace_id),
        idx + 1,
        total,
        sanitize_text(account_id),
        sanitize_text(reason),
    );
    buffer_trace_line(trace_id, line);
}

pub(crate) fn log_attempt_result(
    trace_id: &str,
    account_id: &str,
    upstream_url: Option<&str>,
    status_code: u16,
    error: Option<&str>,
) {
    let should_mark_error = status_code >= 400 || has_error_text(error);
    if should_mark_error {
        mark_trace_has_error(trace_id);
    }
    let line = format!(
        "ts={} event=ATTEMPT_RESULT trace_id={} account_id={} upstream_url={} status={} code={} error={}",
        current_trace_ts(),
        sanitize_text(trace_id),
        sanitize_text(account_id),
        sanitize_text(upstream_url.unwrap_or("-")),
        status_code,
        sanitize_text(crate::error_codes::code_or_dash(error)),
        sanitize_text(error.unwrap_or("-")),
    );
    buffer_trace_line(trace_id, line);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn log_bridge_result(
    trace_id: &str,
    adapter: &str,
    path: &str,
    is_stream: bool,
    upstream_completion_state: UpstreamCompletionState,
    upstream_completion_error: Option<&str>,
    delivery_state: DeliveryState,
    delivery_error: Option<&str>,
    output_text_len: usize,
    output_tokens: Option<i64>,
    delivered_status_code: Option<u16>,
    upstream_error_hint: Option<&str>,
    upstream_request_id: Option<&str>,
    upstream_cf_ray: Option<&str>,
    upstream_auth_error: Option<&str>,
    upstream_identity_error_code: Option<&str>,
    upstream_content_type: Option<&str>,
    last_sse_event_type: Option<&str>,
) {
    let completion_state = match upstream_completion_state {
        UpstreamCompletionState::NonStream => "non_stream",
        UpstreamCompletionState::TerminalOk => "terminal_ok",
        UpstreamCompletionState::TerminalErr => "terminal_err",
        UpstreamCompletionState::EofWithoutTerminal => "eof_without_terminal",
        UpstreamCompletionState::ReaderError => "reader_error",
    };
    let delivery_state_text = match delivery_state {
        DeliveryState::Delivered => "delivered",
        DeliveryState::ClientDisconnect => "client_disconnect",
        DeliveryState::DeliveryError => "delivery_error",
    };
    let bridge_has_error = delivery_state != DeliveryState::Delivered
        || upstream_completion_state != UpstreamCompletionState::TerminalOk && is_stream
        || has_error_text(upstream_completion_error)
        || has_error_text(upstream_error_hint)
        || has_error_text(upstream_auth_error)
        || has_error_text(upstream_identity_error_code);
    if bridge_has_error {
        mark_trace_has_error(trace_id);
    }
    let line = format!(
        "ts={} event=BRIDGE_RESULT trace_id={} adapter={} path={} stream={} completion_state={} completion_error={} delivery_state={} delivery_error={} output_text_len={} output_tokens={} delivered_status={} upstream_hint={} upstream_request_id={} upstream_cf_ray={} upstream_auth_error={} upstream_identity_error_code={} upstream_content_type={} last_sse_event={}",
        current_trace_ts(),
        sanitize_text(trace_id),
        sanitize_text(adapter),
        sanitize_text(path),
        if is_stream { "true" } else { "false" },
        completion_state,
        sanitize_text(upstream_completion_error.unwrap_or("-")),
        delivery_state_text,
        sanitize_text(delivery_error.unwrap_or("-")),
        output_text_len,
        output_tokens
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        delivered_status_code
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        sanitize_text(upstream_error_hint.unwrap_or("-")),
        sanitize_text(upstream_request_id.unwrap_or("-")),
        sanitize_text(upstream_cf_ray.unwrap_or("-")),
        sanitize_text(upstream_auth_error.unwrap_or("-")),
        sanitize_text(upstream_identity_error_code.unwrap_or("-")),
        sanitize_text(upstream_content_type.unwrap_or("-")),
        sanitize_text(last_sse_event_type.unwrap_or("-")),
    );
    buffer_trace_line(trace_id, line);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn log_attempt_profile(
    trace_id: &str,
    account_id: &str,
    candidate_index: usize,
    total: usize,
    strip_session_affinity: bool,
    has_incoming_session: bool,
    has_incoming_turn_state: bool,
    has_incoming_conversation: bool,
    prompt_cache_key: Option<&str>,
    request_shape: Option<&str>,
    body_len: usize,
    body_model: Option<&str>,
) {
    let prompt_cache_key_fp = prompt_cache_key
        .map(short_fingerprint)
        .unwrap_or_else(|| "-".to_string());
    let line = format!(
        "ts={} event=ATTEMPT_PROFILE trace_id={} account_id={} candidate={}/{} strip_session_affinity={} incoming_session={} incoming_turn_state={} incoming_conversation={} prompt_cache_key_fp={} request_shape={} body_len={} body_model={}",
        current_trace_ts(),
        sanitize_text(trace_id),
        sanitize_text(account_id),
        candidate_index + 1,
        total,
        if strip_session_affinity { "true" } else { "false" },
        if has_incoming_session { "true" } else { "false" },
        if has_incoming_turn_state { "true" } else { "false" },
        if has_incoming_conversation { "true" } else { "false" },
        sanitize_text(prompt_cache_key_fp.as_str()),
        sanitize_text(request_shape.unwrap_or("-")),
        body_len,
        sanitize_text(body_model.unwrap_or("-")),
    );
    buffer_trace_line(trace_id, line);
}

pub(crate) fn log_request_final(
    trace_id: &str,
    status_code: u16,
    final_account_id: Option<&str>,
    upstream_url: Option<&str>,
    error: Option<&str>,
    elapsed_ms: u128,
) {
    let should_mark_error = status_code >= 400 || has_error_text(error);
    if should_mark_error {
        mark_trace_has_error(trace_id);
        return;
    }
    let _ = (final_account_id, upstream_url, elapsed_ms);
    clear_trace_state(trace_id);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn log_failed_request(
    ts: i64,
    trace_id: Option<&str>,
    key_id: Option<&str>,
    account_id: Option<&str>,
    method: &str,
    request_path: &str,
    original_path: Option<&str>,
    adapted_path: Option<&str>,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    upstream_url: Option<&str>,
    status_code: Option<u16>,
    error: Option<&str>,
    duration_ms: Option<i64>,
) {
    if !status_code.is_some_and(|status| status >= 400) && !has_error_text(error) {
        return;
    }
    if let Some(trace_id) = trace_id {
        flush_trace_lines(trace_id);
    }
    let code = crate::error_codes::code_or_dash(error);
    let line = format!(
        "ts={ts} event=FAILED_REQUEST trace_id={} key_id={} account_id={} method={} request_path={} original_path={} adapted_path={} model={} reasoning={} upstream_url={} status={} elapsed_ms={} code={} error={}",
        sanitize_text(trace_id.unwrap_or("-")),
        sanitize_text(key_id.unwrap_or("-")),
        sanitize_text(account_id.unwrap_or("-")),
        sanitize_text(method),
        sanitize_text(request_path),
        sanitize_text(original_path.unwrap_or("-")),
        sanitize_text(adapted_path.unwrap_or("-")),
        sanitize_text(model.unwrap_or("-")),
        sanitize_text(reasoning_effort.unwrap_or("-")),
        sanitize_text(upstream_url.unwrap_or("-")),
        status_code
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        duration_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        sanitize_text(code),
        sanitize_text(error.unwrap_or("-")),
    );
    append_trace_line(line, true);
    if let Some(trace_id) = trace_id {
        clear_trace_state(trace_id);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        clear_trace_error, has_error_text, log_failed_request, mark_trace_has_error,
        trace_has_error,
    };

    #[test]
    fn has_error_text_ignores_empty_and_dash() {
        assert!(!has_error_text(None));
        assert!(!has_error_text(Some("")));
        assert!(!has_error_text(Some(" - ")));
        assert!(has_error_text(Some("upstream failed")));
    }

    #[test]
    fn trace_error_state_can_mark_and_clear() {
        let trace_id = "trc_trace_log_unit";
        clear_trace_error(trace_id);
        assert!(!trace_has_error(trace_id));
        mark_trace_has_error(trace_id);
        assert!(trace_has_error(trace_id));
        clear_trace_error(trace_id);
        assert!(!trace_has_error(trace_id));
    }

    #[test]
    fn request_record_ignores_success_without_error() {
        log_failed_request(
            1_772_000_000,
            Some("trc_success"),
            Some("gk_success"),
            Some("acc_success"),
            "POST",
            "/v1/responses",
            Some("/v1/responses"),
            Some("/v1/responses"),
            Some("gpt-5.4"),
            Some("high"),
            Some("https://chatgpt.com/backend-api/codex/responses"),
            Some(200),
            None,
            Some(18),
        );
    }
}
