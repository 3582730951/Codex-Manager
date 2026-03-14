use super::{
    apply_openai_stream_meta_defaults, build_completion_fallback_text_chunk,
    classify_upstream_stream_read_error, collector_output_text_trimmed,
    convert_openai_completions_stream_chunk, extract_openai_completed_output_text,
    extract_sse_frame_payload, inspect_sse_frame, is_response_completed_event_name,
    map_chunk_has_completion_text, mark_collector_terminal_success, merge_usage,
    parse_sse_frame_json, should_skip_completion_live_text_event, sse_keepalive_interval,
    stream_incomplete_message, stream_reader_disconnected_message, update_openai_stream_meta, Arc,
    Cursor, Mutex, OpenAIStreamMeta, PassthroughSseCollector, Read, SseKeepAliveFrame, SseTerminal,
    UpstreamSseFramePump, UpstreamSseFramePumpItem, Value,
};

pub(crate) struct OpenAICompletionsSseReader {
    upstream: UpstreamSseFramePump,
    out_cursor: Cursor<Vec<u8>>,
    usage_collector: Arc<Mutex<PassthroughSseCollector>>,
    stream_meta: OpenAIStreamMeta,
    emitted_text_delta: bool,
    finished: bool,
}

impl OpenAICompletionsSseReader {
    pub(crate) fn new(
        upstream: reqwest::blocking::Response,
        usage_collector: Arc<Mutex<PassthroughSseCollector>>,
    ) -> Self {
        Self {
            upstream: UpstreamSseFramePump::new(upstream),
            out_cursor: Cursor::new(Vec::new()),
            usage_collector,
            stream_meta: OpenAIStreamMeta::default(),
            emitted_text_delta: false,
            finished: false,
        }
    }

    fn update_usage_from_frame(&self, lines: &[String]) {
        let inspection = inspect_sse_frame(lines);
        if inspection.usage.is_none() && inspection.terminal.is_none() {
            return;
        }
        if let Ok(mut collector) = self.usage_collector.lock() {
            if let Some(parsed) = inspection.usage {
                merge_usage(&mut collector.usage, parsed);
            }
            if let Some(terminal) = inspection.terminal {
                collector.saw_terminal = true;
                if let SseTerminal::Err(message) = terminal {
                    collector.terminal_error = Some(message);
                }
            }
        }
    }

    fn try_build_completion_fallback_stream(&mut self, include_done: bool) -> Option<Vec<u8>> {
        if self.emitted_text_delta {
            return None;
        }
        let fallback_text = collector_output_text_trimmed(&self.usage_collector)?;
        let mut fallback_chunk =
            build_completion_fallback_text_chunk(&self.stream_meta, fallback_text.as_str());
        apply_openai_stream_meta_defaults(&mut fallback_chunk, &self.stream_meta);
        let payload = serde_json::to_string(&fallback_chunk).unwrap_or_else(|_| "{}".to_string());
        let mut out = format!("data: {payload}\n\n");
        self.emitted_text_delta = true;
        if include_done {
            out.push_str("data: [DONE]\n\n");
            self.finished = true;
        }
        mark_collector_terminal_success(&self.usage_collector);
        Some(out.into_bytes())
    }

    fn map_frame_to_completions_sse(&mut self, lines: &[String]) -> Vec<u8> {
        let Some(data) = extract_sse_frame_payload(lines) else {
            return Vec::new();
        };
        if data.trim() == "[DONE]" {
            if let Some(fallback) = self.try_build_completion_fallback_stream(true) {
                return fallback;
            }
            self.finished = true;
            return b"data: [DONE]\n\n".to_vec();
        }

        let Some(value) = parse_sse_frame_json(lines) else {
            return Vec::new();
        };
        update_openai_stream_meta(&mut self.stream_meta, &value);
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if event_type == "response.created" {
            return Vec::new();
        }

        let mut out = String::new();
        if is_response_completed_event_name(event_type) && !self.emitted_text_delta {
            if let Some(fallback_text) = extract_openai_completed_output_text(&value) {
                let mut fallback_chunk =
                    build_completion_fallback_text_chunk(&self.stream_meta, fallback_text.as_str());
                apply_openai_stream_meta_defaults(&mut fallback_chunk, &self.stream_meta);
                let payload =
                    serde_json::to_string(&fallback_chunk).unwrap_or_else(|_| "{}".to_string());
                out.push_str(format!("data: {payload}\n\n").as_str());
                self.emitted_text_delta = true;
            }
        }

        if should_skip_completion_live_text_event(event_type, &value) {
            return out.into_bytes();
        }

        if let Some(mut mapped) = convert_openai_completions_stream_chunk(&value) {
            apply_openai_stream_meta_defaults(&mut mapped, &self.stream_meta);
            if map_chunk_has_completion_text(&mapped) {
                self.emitted_text_delta = true;
            }
            let payload = serde_json::to_string(&mapped).unwrap_or_else(|_| "{}".to_string());
            out.push_str(format!("data: {payload}\n\n").as_str());
        }

        if is_response_completed_event_name(event_type) {
            out.push_str("data: [DONE]\n\n");
            self.finished = true;
        }

        out.into_bytes()
    }

    fn next_chunk(&mut self) -> std::io::Result<Vec<u8>> {
        loop {
            match self.upstream.recv_timeout(sse_keepalive_interval()) {
                Ok(UpstreamSseFramePumpItem::Frame(frame)) => {
                    self.update_usage_from_frame(&frame);
                    let mapped = self.map_frame_to_completions_sse(&frame);
                    if !mapped.is_empty() {
                        return Ok(mapped);
                    }
                    continue;
                }
                Ok(UpstreamSseFramePumpItem::Eof) => {
                    if let Some(fallback) = self.try_build_completion_fallback_stream(true) {
                        return Ok(fallback);
                    }
                    if let Ok(mut collector) = self.usage_collector.lock() {
                        if !collector.saw_terminal {
                            // 中文注释：对齐最新 Codex SSE 语义：
                            // 仅凭已收到文本不足以判定成功，必须等到真正 terminal 事件。
                            collector
                                .terminal_error
                                .get_or_insert_with(stream_incomplete_message);
                        }
                    }
                    self.finished = true;
                    return Ok(Vec::new());
                }
                Ok(UpstreamSseFramePumpItem::Error(err)) => {
                    if let Ok(mut collector) = self.usage_collector.lock() {
                        collector
                            .terminal_error
                            .get_or_insert_with(|| classify_upstream_stream_read_error(&err));
                    }
                    self.finished = true;
                    return Ok(Vec::new());
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    return Ok(SseKeepAliveFrame::OpenAICompletions.bytes().to_vec());
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    if let Ok(mut collector) = self.usage_collector.lock() {
                        collector
                            .terminal_error
                            .get_or_insert_with(stream_reader_disconnected_message);
                    }
                    self.finished = true;
                    return Ok(Vec::new());
                }
            }
        }
    }
}

impl Read for OpenAICompletionsSseReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let read = self.out_cursor.read(buf)?;
            if read > 0 {
                return Ok(read);
            }
            if self.finished {
                return Ok(0);
            }
            self.out_cursor = Cursor::new(self.next_chunk()?);
        }
    }
}
