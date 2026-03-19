use std::collections::HashSet;
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

static TRACE_WRITER: OnceLock<TraceAsyncWriter> = OnceLock::new();
static TRACE_SEQ: AtomicU64 = AtomicU64::new(1);
static TRACE_ERROR_TRACES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

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

pub(crate) fn log_stream_phase(
    trace_id: Option<&str>,
    adapter: &str,
    phase: &str,
    detail: Option<&str>,
) {
    let Some(trace_id) = trace_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let line = format!(
        "ts={millis} event=STREAM_PHASE trace_id={} adapter={} phase={} detail={}",
        sanitize_text(trace_id),
        sanitize_text(adapter),
        sanitize_text(phase),
        sanitize_text(detail.unwrap_or("-")),
    );
    append_trace_line(line, false);
}

fn trace_error_traces() -> &'static Mutex<HashSet<String>> {
    TRACE_ERROR_TRACES.get_or_init(|| Mutex::new(HashSet::new()))
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
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let line = format!(
        "ts={millis} event=REQUEST_START trace_id={} key_id={} method={} path={} model={} reasoning={} is_stream={} protocol={}",
        sanitize_text(trace_id),
        sanitize_text(key_id),
        sanitize_text(method),
        sanitize_text(path),
        sanitize_text(model.unwrap_or("-")),
        sanitize_text(reasoning.unwrap_or("-")),
        if is_stream { "1" } else { "0" },
        sanitize_text(protocol_type),
    );
    append_trace_line(line, false);
}

pub(crate) fn log_request_body_preview(trace_id: &str, body: &[u8]) {
    let _ = (trace_id, body, super::trace_body_preview_max_bytes());
}

pub(crate) fn log_request_gate_wait(
    trace_id: &str,
    key_id: &str,
    account_id: &str,
    path: &str,
    model: Option<&str>,
    wait_timeout_ms: u128,
) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let line = format!(
        "ts={millis} event=REQUEST_GATE_WAIT trace_id={} key_id={} account_id={} path={} model={} wait_timeout_ms={}",
        sanitize_text(trace_id),
        sanitize_text(key_id),
        sanitize_text(account_id),
        sanitize_text(path),
        sanitize_text(model.unwrap_or("-")),
        wait_timeout_ms,
    );
    append_trace_line(line, false);
}

pub(crate) fn log_request_gate_acquired(
    trace_id: &str,
    key_id: &str,
    account_id: &str,
    path: &str,
    model: Option<&str>,
    wait_ms: u128,
) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let line = format!(
        "ts={millis} event=REQUEST_GATE_ACQUIRED trace_id={} key_id={} account_id={} path={} model={} wait_ms={}",
        sanitize_text(trace_id),
        sanitize_text(key_id),
        sanitize_text(account_id),
        sanitize_text(path),
        sanitize_text(model.unwrap_or("-")),
        wait_ms,
    );
    append_trace_line(line, false);
}

pub(crate) fn log_request_gate_skip(trace_id: &str, account_id: &str, reason: &str) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let line = format!(
        "ts={millis} event=REQUEST_GATE_SKIP trace_id={} account_id={} reason={}",
        sanitize_text(trace_id),
        sanitize_text(account_id),
        sanitize_text(reason),
    );
    append_trace_line(line, false);
}

pub(crate) fn log_candidate_start(
    trace_id: &str,
    idx: usize,
    total: usize,
    account_id: &str,
    strip_session_affinity: bool,
) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let line = format!(
        "ts={millis} event=CANDIDATE_START trace_id={} idx={} total={} account_id={} strip_session_affinity={}",
        sanitize_text(trace_id),
        idx,
        total,
        sanitize_text(account_id),
        if strip_session_affinity { "1" } else { "0" },
    );
    append_trace_line(line, false);
}

pub(crate) fn log_candidate_pool(
    trace_id: &str,
    key_id: &str,
    strategy: &str,
    candidates: &[String],
) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let candidates_text = if candidates.is_empty() {
        "-".to_string()
    } else {
        sanitize_text(candidates.join(",").as_str())
    };
    let line = format!(
        "ts={millis} event=CANDIDATE_POOL trace_id={} key_id={} strategy={} candidates={}",
        sanitize_text(trace_id),
        sanitize_text(key_id),
        sanitize_text(strategy),
        candidates_text,
    );
    append_trace_line(line, false);
}

pub(crate) fn log_candidate_skip(
    trace_id: &str,
    idx: usize,
    total: usize,
    account_id: &str,
    reason: &str,
) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let line = format!(
        "ts={millis} event=CANDIDATE_SKIP trace_id={} idx={} total={} account_id={} reason={}",
        sanitize_text(trace_id),
        idx,
        total,
        sanitize_text(account_id),
        sanitize_text(reason),
    );
    append_trace_line(line, false);
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
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let line = format!(
        "ts={millis} event=ATTEMPT_RESULT trace_id={} account_id={} upstream_url={} status={} code={} error={}",
        sanitize_text(trace_id),
        sanitize_text(account_id),
        sanitize_text(upstream_url.unwrap_or("-")),
        status_code,
        sanitize_text(crate::error_codes::code_or_dash(error)),
        sanitize_text(error.unwrap_or("-")),
    );
    append_trace_line(line, should_mark_error);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn log_bridge_result(
    trace_id: &str,
    adapter: &str,
    path: &str,
    is_stream: bool,
    stream_terminal_seen: bool,
    stream_terminal_error: Option<&str>,
    delivery_error: Option<&str>,
    bridge_error: Option<&str>,
    output_text_len: usize,
    output_tokens: Option<i64>,
) {
    let bridge_has_error = delivery_error.is_some()
        || bridge_error.is_some()
        || stream_terminal_error.is_some()
        || (is_stream && !stream_terminal_seen);
    if bridge_has_error {
        mark_trace_has_error(trace_id);
    }
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let line = format!(
        "ts={millis} event=BRIDGE_RESULT trace_id={} adapter={} path={} is_stream={} terminal_seen={} terminal_error={} delivery_error={} bridge_error={} output_text_len={} output_tokens={}",
        sanitize_text(trace_id),
        sanitize_text(adapter),
        sanitize_text(path),
        if is_stream { "1" } else { "0" },
        if stream_terminal_seen { "1" } else { "0" },
        sanitize_text(stream_terminal_error.unwrap_or("-")),
        sanitize_text(delivery_error.unwrap_or("-")),
        sanitize_text(bridge_error.unwrap_or("-")),
        output_text_len,
        output_tokens
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
    );
    append_trace_line(line, bridge_has_error);
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
    let _ = (
        trace_id,
        account_id,
        candidate_index,
        total,
        strip_session_affinity,
        has_incoming_session,
        has_incoming_turn_state,
        has_incoming_conversation,
        prompt_cache_key,
        request_shape,
        body_len,
        body_model,
        short_fingerprint,
    );
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
    }
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let line = format!(
        "ts={millis} event=REQUEST_FINAL trace_id={} account_id={} upstream_url={} status={} elapsed_ms={} code={} error={}",
        sanitize_text(trace_id),
        sanitize_text(final_account_id.unwrap_or("-")),
        sanitize_text(upstream_url.unwrap_or("-")),
        status_code,
        elapsed_ms,
        sanitize_text(crate::error_codes::code_or_dash(error)),
        sanitize_text(error.unwrap_or("-")),
    );
    append_trace_line(line, true);
    clear_trace_error(trace_id);
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
