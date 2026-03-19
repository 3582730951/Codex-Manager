use super::{
    apply_openai_stream_meta_defaults, build_chat_fallback_content_chunk,
    classify_upstream_stream_read_error, collector_output_text_trimmed,
    convert_openai_chat_stream_chunk_with_tool_name_restore_map,
    extract_openai_completed_output_text, extract_sse_frame_payload, inspect_sse_frame,
    is_response_completed_event_name, map_chunk_has_chat_text, mark_collector_terminal_success,
    merge_usage, normalize_chat_chunk_delta_role, parse_sse_frame_json,
    should_skip_chat_live_text_event, stream_idle_timeout_reached, stream_incomplete_message,
    stream_poll_timeout, stream_reader_disconnected_message, stream_reader_idle_timeout_message,
    update_openai_stream_meta, Arc, Cursor, Mutex,
    OpenAIStreamMeta, PassthroughSseCollector, Read, SseKeepAliveFrame, SseTerminal,
    ToolNameRestoreMap, UpstreamSseFramePump, UpstreamSseFramePumpItem, Value,
};
use std::time::Instant;

pub(crate) struct OpenAIChatCompletionsSseReader {
    upstream: UpstreamSseFramePump,
    out_cursor: Cursor<Vec<u8>>,
    usage_collector: Arc<Mutex<PassthroughSseCollector>>,
    tool_name_restore_map: Option<ToolNameRestoreMap>,
    trace_id: Option<String>,
    stream_meta: OpenAIStreamMeta,
    emitted_text_delta: bool,
    emitted_assistant_role: bool,
    saw_upstream_frame: bool,
    saw_semantic_output: bool,
    last_upstream_activity: Instant,
    finished: bool,
}

impl OpenAIChatCompletionsSseReader {
    pub(crate) fn new(
        upstream: reqwest::blocking::Response,
        usage_collector: Arc<Mutex<PassthroughSseCollector>>,
        tool_name_restore_map: Option<ToolNameRestoreMap>,
        trace_id: Option<&str>,
    ) -> Self {
        Self {
            upstream: UpstreamSseFramePump::new(upstream),
            out_cursor: Cursor::new(Vec::new()),
            usage_collector,
            tool_name_restore_map,
            trace_id: trace_id.map(str::to_string),
            stream_meta: OpenAIStreamMeta::default(),
            emitted_text_delta: false,
            emitted_assistant_role: false,
            saw_upstream_frame: false,
            saw_semantic_output: false,
            last_upstream_activity: Instant::now(),
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
                crate::gateway::trace_log::log_stream_phase(
                    self.trace_id.as_deref(),
                    "openai_chat",
                    "terminal_seen",
                    collector.terminal_error.as_deref().or(Some("ok")),
                );
            }
        }
    }

    fn try_build_chat_fallback_stream(&mut self, include_done: bool) -> Option<Vec<u8>> {
        if self.emitted_text_delta {
            return None;
        }
        let fallback_content = collector_output_text_trimmed(&self.usage_collector)?;
        let mut fallback_chunk =
            build_chat_fallback_content_chunk(&self.stream_meta, fallback_content.as_str());
        apply_openai_stream_meta_defaults(&mut fallback_chunk, &self.stream_meta);
        normalize_chat_chunk_delta_role(&mut fallback_chunk, &mut self.emitted_assistant_role);
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

    fn map_frame_to_chat_completions_sse(&mut self, lines: &[String]) -> Vec<u8> {
        let Some(data) = extract_sse_frame_payload(lines) else {
            return Vec::new();
        };
        if data.trim() == "[DONE]" {
            if let Some(fallback) = self.try_build_chat_fallback_stream(true) {
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
            if let Some(fallback_content) = extract_openai_completed_output_text(&value) {
                let mut fallback_chunk =
                    build_chat_fallback_content_chunk(&self.stream_meta, fallback_content.as_str());
                apply_openai_stream_meta_defaults(&mut fallback_chunk, &self.stream_meta);
                normalize_chat_chunk_delta_role(
                    &mut fallback_chunk,
                    &mut self.emitted_assistant_role,
                );
                let payload =
                    serde_json::to_string(&fallback_chunk).unwrap_or_else(|_| "{}".to_string());
                out.push_str(format!("data: {payload}\n\n").as_str());
                self.emitted_text_delta = true;
            }
        }

        if should_skip_chat_live_text_event(event_type, &value) {
            return out.into_bytes();
        }

        if let Some(mut mapped) = convert_openai_chat_stream_chunk_with_tool_name_restore_map(
            &value,
            self.tool_name_restore_map.as_ref(),
        ) {
            apply_openai_stream_meta_defaults(&mut mapped, &self.stream_meta);
            normalize_chat_chunk_delta_role(&mut mapped, &mut self.emitted_assistant_role);
            if map_chunk_has_chat_text(&mapped) {
                self.emitted_text_delta = true;
            }
            let payload = serde_json::to_string(&mapped).unwrap_or_else(|_| "{}".to_string());
            out.push_str(format!("data: {payload}\n\n").as_str());
        }

        if is_response_completed_event_name(event_type) {
            out.push_str("data: [DONE]\n\n");
            self.finished = true;
        }

        if !self.saw_semantic_output
            && !out.trim().is_empty()
            && out.trim() != "data: [DONE]"
        {
            self.saw_semantic_output = true;
            crate::gateway::trace_log::log_stream_phase(
                self.trace_id.as_deref(),
                "openai_chat",
                "first_semantic_event",
                Some(event_type),
            );
        }

        out.into_bytes()
    }

    fn next_chunk(&mut self) -> std::io::Result<Vec<u8>> {
        loop {
            if stream_idle_timeout_reached(self.last_upstream_activity) {
                if let Ok(mut collector) = self.usage_collector.lock() {
                    collector
                        .terminal_error
                        .get_or_insert_with(stream_reader_idle_timeout_message);
                }
                crate::gateway::trace_log::log_stream_phase(
                    self.trace_id.as_deref(),
                    "openai_chat",
                    "idle_timeout_triggered",
                    None,
                );
                self.finished = true;
                return Ok(Vec::new());
            }
            match self
                .upstream
                .recv_timeout(stream_poll_timeout(self.last_upstream_activity))
            {
                Ok(UpstreamSseFramePumpItem::Frame(frame)) => {
                    self.last_upstream_activity = Instant::now();
                    if !self.saw_upstream_frame {
                        self.saw_upstream_frame = true;
                        crate::gateway::trace_log::log_stream_phase(
                            self.trace_id.as_deref(),
                            "openai_chat",
                            "first_upstream_frame",
                            None,
                        );
                    }
                    self.update_usage_from_frame(&frame);
                    let mapped = self.map_frame_to_chat_completions_sse(&frame);
                    if !mapped.is_empty() {
                        return Ok(mapped);
                    }
                    continue;
                }
                Ok(UpstreamSseFramePumpItem::Eof) => {
                    if let Some(fallback) = self.try_build_chat_fallback_stream(true) {
                        return Ok(fallback);
                    }
                    if let Ok(mut collector) = self.usage_collector.lock() {
                        if !collector.saw_terminal {
                            // 中文注释：对齐最新 Codex SSE 语义：
                            // 只有 response.completed / response.done / [DONE] 才算正常结束。
                            collector
                                .terminal_error
                                .get_or_insert_with(stream_incomplete_message);
                            crate::gateway::trace_log::log_stream_phase(
                                self.trace_id.as_deref(),
                                "openai_chat",
                                "eof_without_terminal",
                                collector.terminal_error.as_deref(),
                            );
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
                    crate::gateway::trace_log::log_stream_phase(
                        self.trace_id.as_deref(),
                        "openai_chat",
                        "read_error",
                        Some(err.as_str()),
                    );
                    self.finished = true;
                    return Ok(Vec::new());
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if stream_idle_timeout_reached(self.last_upstream_activity) {
                        if let Ok(mut collector) = self.usage_collector.lock() {
                            collector
                                .terminal_error
                                .get_or_insert_with(stream_reader_idle_timeout_message);
                        }
                        crate::gateway::trace_log::log_stream_phase(
                            self.trace_id.as_deref(),
                            "openai_chat",
                            "idle_timeout_triggered",
                            None,
                        );
                        self.finished = true;
                        return Ok(Vec::new());
                    }
                    crate::gateway::trace_log::log_stream_phase(
                        self.trace_id.as_deref(),
                        "openai_chat",
                        "keepalive_emitted",
                        None,
                    );
                    return Ok(SseKeepAliveFrame::OpenAIChatCompletions.bytes().to_vec());
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    if let Ok(mut collector) = self.usage_collector.lock() {
                        collector
                            .terminal_error
                            .get_or_insert_with(stream_reader_disconnected_message);
                    }
                    crate::gateway::trace_log::log_stream_phase(
                        self.trace_id.as_deref(),
                        "openai_chat",
                        "reader_disconnected",
                        None,
                    );
                    self.finished = true;
                    return Ok(Vec::new());
                }
            }
        }
    }
}

impl Read for OpenAIChatCompletionsSseReader {
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
