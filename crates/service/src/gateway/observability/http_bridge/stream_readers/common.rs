use super::{Arc, Mutex, UpstreamCompletionState, UpstreamResponseUsage};
use std::io::{BufRead, BufReader, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

const DEFAULT_SSE_KEEPALIVE_INTERVAL_MS: u64 = 15_000;
const ENV_SSE_KEEPALIVE_INTERVAL_MS: &str = "CODEXMANAGER_SSE_KEEPALIVE_INTERVAL_MS";
const LEGACY_SSE_FRAME_PUMP_CHANNEL_CAPACITY: usize = 32;
const SSE_FRAME_PUMP_READ_CHUNK_BYTES: usize = 8192;

static SSE_KEEPALIVE_INTERVAL_MS: AtomicU64 = AtomicU64::new(DEFAULT_SSE_KEEPALIVE_INTERVAL_MS);

#[derive(Debug, Clone, Default)]
pub(crate) struct PassthroughSseCollector {
    pub(crate) usage: UpstreamResponseUsage,
    pub(crate) saw_terminal: bool,
    pub(crate) completion_state: Option<UpstreamCompletionState>,
    pub(crate) terminal_error: Option<String>,
    pub(crate) upstream_error_hint: Option<String>,
    pub(crate) last_event_type: Option<String>,
    pub(crate) raw_sse_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SseKeepAliveFrame {
    Comment,
    OpenAIResponses,
    OpenAIChatCompletions,
    OpenAICompletions,
    Anthropic,
}

impl SseKeepAliveFrame {
    pub(crate) fn bytes(self) -> &'static [u8] {
        match self {
            Self::Comment => b": keep-alive\n\n",
            Self::OpenAIResponses => b"data: {\"type\":\"codexmanager.keepalive\"}\n\n",
            Self::OpenAIChatCompletions => {
                b"data: {\"id\":\"cm_keepalive\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"codexmanager.keepalive\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":null}]}\n\n"
            }
            Self::OpenAICompletions => {
                b"data: {\"id\":\"cm_keepalive\",\"object\":\"text_completion\",\"created\":0,\"model\":\"codexmanager.keepalive\",\"choices\":[{\"index\":0,\"text\":\"\",\"finish_reason\":null}]}\n\n"
            }
            Self::Anthropic => {
                b"event: ping\ndata: {\"type\":\"ping\"}\n\n"
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum UpstreamSseFramePumpItem {
    Frame(Vec<String>),
    Eof,
    Error(String),
}

pub(crate) struct UpstreamSseFramePump {
    rx: Receiver<UpstreamSseFramePumpItem>,
}

impl UpstreamSseFramePump {
    pub(crate) fn new(upstream: reqwest::blocking::Response) -> Self {
        let use_v2 = crate::gateway::experimental_sse_frame_pump_v2_enabled();
        let channel_capacity = if use_v2 {
            crate::gateway::current_stream_pump_channel_capacity()
        } else {
            LEGACY_SSE_FRAME_PUMP_CHANNEL_CAPACITY
        };
        let (tx, rx) = mpsc::sync_channel::<UpstreamSseFramePumpItem>(channel_capacity);
        if use_v2 {
            let stack_size =
                crate::gateway::current_stream_pump_thread_stack_kb().saturating_mul(1024);
            if let Err(err) = thread::Builder::new()
                .name("gateway-sse-pump".to_string())
                .stack_size(stack_size)
                .spawn(move || run_v2_frame_pump(upstream, tx))
            {
                log::warn!("event=gateway_sse_frame_pump_spawn_failed err={}", err);
            }
        } else {
            thread::spawn(move || run_legacy_frame_pump(upstream, tx));
        }
        Self { rx }
    }

    pub(crate) fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<UpstreamSseFramePumpItem, RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }
}

fn run_legacy_frame_pump(
    upstream: reqwest::blocking::Response,
    tx: mpsc::SyncSender<UpstreamSseFramePumpItem>,
) {
    let mut reader = BufReader::new(upstream);
    let mut pending_frame_lines = Vec::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                if !pending_frame_lines.is_empty() {
                    crate::gateway::record_gateway_stream_pump_frame();
                    if tx
                        .send(UpstreamSseFramePumpItem::Frame(pending_frame_lines))
                        .is_err()
                    {
                        return;
                    }
                }
                let _ = tx.send(UpstreamSseFramePumpItem::Eof);
                return;
            }
            Ok(_) => {
                let is_blank = line == "\n" || line == "\r\n";
                pending_frame_lines.push(line);
                if is_blank {
                    let frame = std::mem::take(&mut pending_frame_lines);
                    crate::gateway::record_gateway_stream_pump_frame();
                    if tx.send(UpstreamSseFramePumpItem::Frame(frame)).is_err() {
                        return;
                    }
                }
            }
            Err(err) => {
                crate::gateway::record_gateway_stream_pump_disconnect();
                let _ = tx.send(UpstreamSseFramePumpItem::Error(err.to_string()));
                return;
            }
        }
    }
}

fn run_v2_frame_pump(
    upstream: reqwest::blocking::Response,
    tx: mpsc::SyncSender<UpstreamSseFramePumpItem>,
) {
    let mut reader = BufReader::with_capacity(SSE_FRAME_PUMP_READ_CHUNK_BYTES, upstream);
    let mut chunk = [0_u8; SSE_FRAME_PUMP_READ_CHUNK_BYTES];
    let mut pending = Vec::with_capacity(SSE_FRAME_PUMP_READ_CHUNK_BYTES * 2);
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => {
                if !pending.is_empty() {
                    crate::gateway::record_gateway_stream_pump_frame();
                    if tx
                        .send(UpstreamSseFramePumpItem::Frame(frame_bytes_to_lines(
                            pending.as_slice(),
                        )))
                        .is_err()
                    {
                        return;
                    }
                }
                let _ = tx.send(UpstreamSseFramePumpItem::Eof);
                return;
            }
            Ok(read) => {
                pending.extend_from_slice(&chunk[..read]);
                while let Some(frame) = take_next_frame_bytes(&mut pending) {
                    crate::gateway::record_gateway_stream_pump_frame();
                    if tx
                        .send(UpstreamSseFramePumpItem::Frame(frame_bytes_to_lines(
                            frame.as_slice(),
                        )))
                        .is_err()
                    {
                        return;
                    }
                }
            }
            Err(err) => {
                crate::gateway::record_gateway_stream_pump_disconnect();
                let _ = tx.send(UpstreamSseFramePumpItem::Error(err.to_string()));
                return;
            }
        }
    }
}

fn take_next_frame_bytes(pending: &mut Vec<u8>) -> Option<Vec<u8>> {
    let boundary = find_frame_boundary(pending.as_slice())?;
    Some(pending.drain(..boundary).collect())
}

fn find_frame_boundary(input: &[u8]) -> Option<usize> {
    let mut idx = 0usize;
    while idx + 1 < input.len() {
        if input[idx] == b'\n' && input[idx + 1] == b'\n' {
            return Some(idx + 2);
        }
        if idx + 3 < input.len() && &input[idx..idx + 4] == b"\r\n\r\n" {
            return Some(idx + 4);
        }
        idx += 1;
    }
    None
}

fn frame_bytes_to_lines(frame: &[u8]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for (idx, byte) in frame.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(String::from_utf8_lossy(&frame[start..=idx]).into_owned());
            start = idx + 1;
        }
    }
    if start < frame.len() {
        lines.push(String::from_utf8_lossy(&frame[start..]).into_owned());
    }
    if lines.is_empty() && !frame.is_empty() {
        lines.push(String::from_utf8_lossy(frame).into_owned());
    }
    lines
}

pub(super) fn reload_from_env() {
    SSE_KEEPALIVE_INTERVAL_MS.store(
        std::env::var(ENV_SSE_KEEPALIVE_INTERVAL_MS)
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(DEFAULT_SSE_KEEPALIVE_INTERVAL_MS),
        Ordering::Relaxed,
    );
}

pub(super) fn sse_keepalive_interval() -> Duration {
    let interval_ms = SSE_KEEPALIVE_INTERVAL_MS.load(Ordering::Relaxed);
    Duration::from_millis(interval_ms.max(1))
}

pub(super) fn current_sse_keepalive_interval_ms() -> u64 {
    SSE_KEEPALIVE_INTERVAL_MS.load(Ordering::Relaxed).max(1)
}

pub(super) fn set_sse_keepalive_interval_ms(interval_ms: u64) -> Result<u64, String> {
    if interval_ms == 0 {
        return Err("SSE keepalive interval must be greater than 0".to_string());
    }
    SSE_KEEPALIVE_INTERVAL_MS.store(interval_ms, Ordering::Relaxed);
    std::env::set_var(ENV_SSE_KEEPALIVE_INTERVAL_MS, interval_ms.to_string());
    Ok(interval_ms)
}

pub(super) fn collector_output_text_trimmed(
    usage_collector: &Arc<Mutex<PassthroughSseCollector>>,
) -> Option<String> {
    usage_collector
        .lock()
        .ok()
        .and_then(|collector| collector.usage.output_text.clone())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

pub(super) fn mark_collector_terminal_success(
    usage_collector: &Arc<Mutex<PassthroughSseCollector>>,
) {
    if let Ok(mut collector) = usage_collector.lock() {
        collector.saw_terminal = true;
        collector.completion_state = Some(UpstreamCompletionState::TerminalOk);
        collector.terminal_error = None;
    }
}

pub(super) fn mark_collector_terminal_error(
    usage_collector: &Arc<Mutex<PassthroughSseCollector>>,
    state: UpstreamCompletionState,
    message: String,
) {
    if let Ok(mut collector) = usage_collector.lock() {
        collector.saw_terminal = matches!(
            state,
            UpstreamCompletionState::TerminalOk | UpstreamCompletionState::TerminalErr
        );
        collector.completion_state = Some(state);
        collector.terminal_error = Some(message);
    }
}

pub(super) fn stream_incomplete_message() -> String {
    "上游流中途中断（未正常结束）".to_string()
}

pub(super) fn stream_reader_disconnected_message() -> String {
    "上游流读取失败（连接中断）".to_string()
}

pub(super) fn classify_upstream_stream_read_error(raw: &str) -> String {
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return "上游流读取失败".to_string();
    }
    if normalized.contains("timed out") || normalized.contains("timeout") {
        return "上游请求超时".to_string();
    }
    if normalized.contains("broken pipe")
        || normalized.contains("connection reset")
        || normalized.contains("connection aborted")
        || normalized.contains("forcibly closed")
        || normalized.contains("unexpected eof")
        || normalized.contains("early eof")
    {
        return "上游流读取失败（连接中断）".to_string();
    }
    if normalized.contains("request or response body error")
        || normalized.contains("response body")
        || normalized.contains("error decoding response body")
        || normalized.contains("body error")
    {
        return "上游返回的不是正常接口数据，可能是验证页、拦截页或错误页".to_string();
    }
    "上游流读取失败".to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        classify_upstream_stream_read_error, find_frame_boundary, frame_bytes_to_lines,
        stream_incomplete_message, stream_reader_disconnected_message, take_next_frame_bytes,
    };

    #[test]
    fn classify_upstream_stream_read_error_maps_body_error() {
        assert_eq!(
            classify_upstream_stream_read_error("request or response body error"),
            "上游返回的不是正常接口数据，可能是验证页、拦截页或错误页"
        );
    }

    #[test]
    fn classify_upstream_stream_read_error_maps_disconnect() {
        assert_eq!(
            classify_upstream_stream_read_error("connection reset by peer"),
            "上游流读取失败（连接中断）"
        );
    }

    #[test]
    fn classify_upstream_stream_read_error_maps_timeout() {
        assert_eq!(
            classify_upstream_stream_read_error("operation timed out"),
            "上游请求超时"
        );
    }

    #[test]
    fn stream_terminal_messages_are_user_friendly() {
        assert_eq!(stream_incomplete_message(), "上游流中途中断（未正常结束）");
        assert_eq!(
            stream_reader_disconnected_message(),
            "上游流读取失败（连接中断）"
        );
    }

    #[test]
    fn find_frame_boundary_detects_lf_delimiter() {
        assert_eq!(find_frame_boundary(b"data: one\n\ndata: two"), Some(11));
    }

    #[test]
    fn take_next_frame_bytes_splits_crlf_delimiter() {
        let mut input = b"data: one\r\n\r\ndata: two\r\n\r\n".to_vec();
        let first = take_next_frame_bytes(&mut input).expect("first frame");
        assert_eq!(String::from_utf8_lossy(&first), "data: one\r\n\r\n");
        assert_eq!(String::from_utf8_lossy(&input), "data: two\r\n\r\n");
    }

    #[test]
    fn frame_bytes_to_lines_preserves_newlines() {
        let lines = frame_bytes_to_lines(b"event: message\ndata: ok\n\n");
        assert_eq!(lines[0], "event: message\n");
        assert_eq!(lines[1], "data: ok\n");
        assert_eq!(lines[2], "\n");
    }
}
