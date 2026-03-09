use super::*;

#[derive(Debug, Clone, Default)]
pub(crate) struct PassthroughSseCollector {
    pub(crate) usage: UpstreamResponseUsage,
    pub(crate) saw_terminal: bool,
    pub(crate) terminal_error: Option<String>,
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
        collector.terminal_error = None;
    }
}