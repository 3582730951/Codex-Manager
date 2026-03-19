use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Request, Response, StatusCode};

use super::super::{
    adapt_upstream_response, adapt_upstream_response_with_tool_name_restore_map,
    build_anthropic_error_body, ResponseAdapter, ToolNameRestoreMap,
};
use super::{
    collect_non_stream_json_from_sse_bytes, extract_error_hint_from_body, looks_like_sse_payload,
    merge_usage, parse_usage_from_json, push_trace_id_header, usage_has_signal, AnthropicSseReader,
    OpenAIChatCompletionsSseReader, OpenAICompletionsSseReader, PassthroughSseCollector,
    PassthroughSseUsageReader, SseKeepAliveFrame, UpstreamResponseBridgeResult,
    UpstreamResponseUsage,
};

const MAX_SYNTHETIC_SSE_BODY_BYTES: usize = 256 * 1024;

pub(crate) fn respond_with_upstream(
    request: Request,
    upstream: reqwest::blocking::Response,
    _inflight_guard: super::super::AccountInFlightGuard,
    response_adapter: ResponseAdapter,
    request_path: &str,
    tool_name_restore_map: Option<&ToolNameRestoreMap>,
    is_stream: bool,
    trace_id: Option<&str>,
) -> Result<UpstreamResponseBridgeResult, String> {
    let keepalive_frame = resolve_stream_keepalive_frame(response_adapter, request_path);
    match response_adapter {
        ResponseAdapter::Passthrough => {
            let upstream_content_type = upstream
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_string());
            let status = StatusCode(upstream.status().as_u16());
            let mut headers = Vec::new();
            for (name, value) in upstream.headers().iter() {
                let name_str = name.as_str();
                if name_str.eq_ignore_ascii_case("transfer-encoding")
                    || name_str.eq_ignore_ascii_case("content-length")
                    || name_str.eq_ignore_ascii_case("connection")
                {
                    continue;
                }
                if let Ok(header) = Header::from_bytes(name_str.as_bytes(), value.as_bytes()) {
                    headers.push(header);
                }
            }
            if let Some(trace_id) = trace_id {
                push_trace_id_header(&mut headers, trace_id);
            }
            let is_json = upstream_content_type
                .as_deref()
                .map(|value| value.to_ascii_lowercase().contains("application/json"))
                .unwrap_or(false);
            let is_sse = upstream_content_type
                .as_deref()
                .map(|value| value.to_ascii_lowercase().starts_with("text/event-stream"))
                .unwrap_or(false);
            if !is_stream {
                let upstream_body = upstream
                    .bytes()
                    .map_err(|err| format!("read upstream body failed: {err}"))?;
                let detected_sse =
                    is_sse || (!is_json && looks_like_sse_payload(upstream_body.as_ref()));
                if detected_sse {
                    let (synthesized_body, mut usage) =
                        collect_non_stream_json_from_sse_bytes(upstream_body.as_ref());
                    let synthesized_response = synthesized_body.is_some();
                    let body = synthesized_body.unwrap_or_else(|| upstream_body.to_vec());
                    if let Ok(value) = serde_json::from_slice::<Value>(&body) {
                        merge_usage(&mut usage, parse_usage_from_json(&value));
                    }
                    let upstream_error_hint = extract_error_hint_from_body(status.0, &body);
                    if synthesized_response {
                        headers.retain(|header| {
                            !header
                                .field
                                .as_str()
                                .as_str()
                                .eq_ignore_ascii_case("Content-Type")
                        });
                        if let Ok(content_type_header) = Header::from_bytes(
                            b"Content-Type".as_slice(),
                            b"application/json".as_slice(),
                        ) {
                            headers.push(content_type_header);
                        }
                    }
                    let len = Some(body.len());
                    let response =
                        Response::new(status, headers, std::io::Cursor::new(body), len, None);
                    let delivery_error = request.respond(response).err().map(|err| err.to_string());
                    return Ok(UpstreamResponseBridgeResult {
                        usage,
                        stream_terminal_seen: true,
                        stream_terminal_error: None,
                        delivery_error,
                        upstream_error_hint,
                        bridge_error: None,
                    });
                }

                let (_, sse_usage) = collect_non_stream_json_from_sse_bytes(upstream_body.as_ref());
                let usage = if is_json {
                    serde_json::from_slice::<Value>(upstream_body.as_ref())
                        .ok()
                        .map(|value| parse_usage_from_json(&value))
                        .unwrap_or_default()
                } else if usage_has_signal(&sse_usage) {
                    sse_usage
                } else {
                    UpstreamResponseUsage::default()
                };
                let upstream_error_hint =
                    extract_error_hint_from_body(status.0, upstream_body.as_ref());
                let len = Some(upstream_body.len());
                let response = Response::new(
                    status,
                    headers,
                    std::io::Cursor::new(upstream_body.to_vec()),
                    len,
                    None,
                );
                let delivery_error = request.respond(response).err().map(|err| err.to_string());
                return Ok(UpstreamResponseBridgeResult {
                    usage,
                    stream_terminal_seen: true,
                    stream_terminal_error: None,
                    delivery_error,
                    upstream_error_hint,
                    bridge_error: None,
                });
            }
            if is_stream && !is_sse && status.0 >= 400 {
                let upstream_body = upstream
                    .bytes()
                    .map_err(|err| format!("read upstream body failed: {err}"))?;
                let usage = if is_json {
                    serde_json::from_slice::<Value>(upstream_body.as_ref())
                        .ok()
                        .map(|value| parse_usage_from_json(&value))
                        .unwrap_or_default()
                } else {
                    UpstreamResponseUsage::default()
                };
                let upstream_error_hint =
                    extract_error_hint_from_body(status.0, upstream_body.as_ref());
                let len = Some(upstream_body.len());
                let response = Response::new(
                    status,
                    headers,
                    std::io::Cursor::new(upstream_body.to_vec()),
                    len,
                    None,
                );
                let delivery_error = request.respond(response).err().map(|err| err.to_string());
                return Ok(UpstreamResponseBridgeResult {
                    usage,
                    stream_terminal_seen: true,
                    stream_terminal_error: None,
                    delivery_error,
                    upstream_error_hint,
                    bridge_error: None,
                });
            }
            if is_sse || is_stream {
                let usage_collector = Arc::new(Mutex::new(PassthroughSseCollector::default()));
                let response = Response::new(
                    status,
                    headers,
                    PassthroughSseUsageReader::new(
                        upstream,
                        Arc::clone(&usage_collector),
                        keepalive_frame,
                        trace_id,
                    ),
                    None,
                    None,
                );
                let delivery_error = request.respond(response).err().map(|err| err.to_string());
                let collector = usage_collector
                    .lock()
                    .map(|guard| guard.clone())
                    .unwrap_or_default();
                return Ok(UpstreamResponseBridgeResult {
                    usage: collector.usage,
                    stream_terminal_seen: collector.saw_terminal,
                    stream_terminal_error: collector.terminal_error,
                    delivery_error,
                    upstream_error_hint: collector.upstream_error_hint,
                    bridge_error: None,
                });
            }
            let len = upstream.content_length().map(|v| v as usize);
            let response = Response::new(status, headers, upstream, len, None);
            let delivery_error = request.respond(response).err().map(|err| err.to_string());
            Ok(UpstreamResponseBridgeResult {
                usage: UpstreamResponseUsage::default(),
                stream_terminal_seen: true,
                stream_terminal_error: None,
                delivery_error,
                upstream_error_hint: None,
                bridge_error: None,
            })
        }
        ResponseAdapter::OpenAIChatCompletionsJson
        | ResponseAdapter::OpenAIChatCompletionsSse
        | ResponseAdapter::OpenAICompletionsJson
        | ResponseAdapter::OpenAICompletionsSse => {
            let status = StatusCode(upstream.status().as_u16());
            let mut headers = Vec::new();
            for (name, value) in upstream.headers().iter() {
                let name_str = name.as_str();
                if name_str.eq_ignore_ascii_case("transfer-encoding")
                    || name_str.eq_ignore_ascii_case("content-length")
                    || name_str.eq_ignore_ascii_case("connection")
                    || name_str.eq_ignore_ascii_case("content-type")
                {
                    continue;
                }
                if let Ok(header) = Header::from_bytes(name_str.as_bytes(), value.as_bytes()) {
                    headers.push(header);
                }
            }
            if let Some(trace_id) = trace_id {
                push_trace_id_header(&mut headers, trace_id);
            }
            let upstream_content_type = upstream
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_string());
            let is_sse = upstream_content_type
                .as_deref()
                .map(|value| value.to_ascii_lowercase().starts_with("text/event-stream"))
                .unwrap_or(false);
            let use_openai_sse_adapter = matches!(
                response_adapter,
                ResponseAdapter::OpenAIChatCompletionsSse | ResponseAdapter::OpenAICompletionsSse
            );

            if use_openai_sse_adapter && is_stream && !is_sse {
                log::warn!(
                    "event=gateway_openai_stream_content_type_mismatch adapter={:?} upstream_content_type={}",
                    response_adapter,
                    upstream_content_type
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or("-")
                );
                crate::gateway::trace_log::log_stream_phase(
                    trace_id,
                    format!("{response_adapter:?}").as_str(),
                    "content_type_mismatch",
                    upstream_content_type.as_deref(),
                );
            }

            if use_openai_sse_adapter && (is_stream || is_sse) && is_sse {
                if let Ok(content_type_header) =
                    Header::from_bytes(b"Content-Type".as_slice(), b"text/event-stream".as_slice())
                {
                    headers.push(content_type_header);
                }
                let usage_collector = Arc::new(Mutex::new(PassthroughSseCollector::default()));
                let delivery_error =
                    if response_adapter == ResponseAdapter::OpenAIChatCompletionsSse {
                        let response = Response::new(
                            status,
                            headers,
                            OpenAIChatCompletionsSseReader::new(
                                upstream,
                                Arc::clone(&usage_collector),
                                tool_name_restore_map.cloned(),
                                trace_id,
                            ),
                            None,
                            None,
                        );
                        request.respond(response).err().map(|err| err.to_string())
                    } else {
                        let response = Response::new(
                            status,
                            headers,
                            OpenAICompletionsSseReader::new(
                                upstream,
                                Arc::clone(&usage_collector),
                                trace_id,
                            ),
                            None,
                            None,
                        );
                        request.respond(response).err().map(|err| err.to_string())
                    };
                let collector = usage_collector
                    .lock()
                    .map(|guard| guard.clone())
                    .unwrap_or_default();
                let output_text_empty = collector
                    .usage
                    .output_text
                    .as_deref()
                    .map(str::trim)
                    .is_none_or(str::is_empty);
                if output_text_empty {
                    log::warn!(
                        "event=gateway_openai_stream_empty_output adapter={:?} terminal_seen={} terminal_error={} output_tokens={:?}",
                        response_adapter,
                        collector.saw_terminal,
                        collector.terminal_error.as_deref().unwrap_or("-"),
                        collector.usage.output_tokens
                    );
                }
                return Ok(UpstreamResponseBridgeResult {
                    usage: collector.usage,
                    stream_terminal_seen: collector.saw_terminal,
                    stream_terminal_error: collector.terminal_error,
                    delivery_error,
                    upstream_error_hint: None,
                    bridge_error: None,
                });
            }

            let upstream_body = upstream
                .bytes()
                .map_err(|err| format!("read upstream body failed: {err}"))?;
            let mut usage = if is_sse {
                let (_, parsed) = collect_non_stream_json_from_sse_bytes(upstream_body.as_ref());
                parsed
            } else {
                UpstreamResponseUsage::default()
            };
            if let Ok(value) = serde_json::from_slice::<Value>(upstream_body.as_ref()) {
                merge_usage(&mut usage, parse_usage_from_json(&value));
            }
            let mut bridge_error = None;
            let (mut body, mut content_type) =
                match adapt_upstream_response_with_tool_name_restore_map(
                    response_adapter,
                    upstream_content_type.as_deref(),
                    upstream_body.as_ref(),
                    tool_name_restore_map,
                ) {
                    Ok(result) => result,
                    Err(err) => {
                        bridge_error = Some(format!("response conversion failed: {err}"));
                        (
                            serde_json::to_vec(&json!({
                                "error": {
                                    "message": bridge_error.clone().unwrap_or_else(|| "response conversion failed".to_string()),
                                    "type": "server_error"
                                }
                            }))
                            .unwrap_or_else(|_| {
                                b"{\"error\":{\"message\":\"response conversion failed\",\"type\":\"server_error\"}}"
                                    .to_vec()
                            }),
                            "application/json",
                        )
                    }
                };
            if use_openai_sse_adapter
                && is_stream
                && status.0 < 400
                && !content_type.eq_ignore_ascii_case("text/event-stream")
            {
                let upstream_content_type_label = upstream_content_type
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("-");
                if body.len() <= MAX_SYNTHETIC_SSE_BODY_BYTES {
                    if let Ok(mapped_json) = serde_json::from_slice::<Value>(body.as_ref()) {
                        merge_usage(&mut usage, parse_usage_from_json(&mapped_json));
                        body = if response_adapter == ResponseAdapter::OpenAIChatCompletionsSse {
                            super::synthesize_chat_completion_sse_from_json(&mapped_json)
                        } else {
                            super::synthesize_completions_sse_from_json(&mapped_json)
                        };
                        content_type = "text/event-stream";
                        log::warn!(
                            "event=gateway_openai_stream_synthetic_sse adapter={:?} status={} upstream_content_type={}",
                            response_adapter,
                            status.0,
                            upstream_content_type_label
                        );
                        crate::gateway::trace_log::log_stream_phase(
                            trace_id,
                            format!("{response_adapter:?}").as_str(),
                            "synthetic_sse_fallback",
                            upstream_content_type.as_deref(),
                        );
                    }
                } else {
                    let body_len_detail = body.len().to_string();
                    crate::gateway::trace_log::log_stream_phase(
                        trace_id,
                        format!("{response_adapter:?}").as_str(),
                        "synthetic_sse_skipped_oversize",
                        Some(body_len_detail.as_str()),
                    );
                }
                if !content_type.eq_ignore_ascii_case("text/event-stream") && bridge_error.is_none() {
                    bridge_error = Some(format!(
                        "gateway_bridge_error: content_type_mismatch upstream_content_type={} mapped_content_type={}",
                        upstream_content_type_label, content_type
                    ));
                }
            }
            if let Ok(content_type_header) =
                Header::from_bytes(b"Content-Type".as_slice(), content_type.as_bytes())
            {
                headers.push(content_type_header);
            }
            let len = Some(body.len());
            let response = Response::new(status, headers, std::io::Cursor::new(body), len, None);
            let delivery_error = request.respond(response).err().map(|err| err.to_string());
            let upstream_error_hint =
                extract_error_hint_from_body(status.0, upstream_body.as_ref());
            Ok(UpstreamResponseBridgeResult {
                usage,
                stream_terminal_seen: true,
                stream_terminal_error: None,
                delivery_error,
                upstream_error_hint,
                bridge_error,
            })
        }
        ResponseAdapter::AnthropicJson | ResponseAdapter::AnthropicSse => {
            let status = StatusCode(upstream.status().as_u16());
            let mut headers = Vec::new();
            for (name, value) in upstream.headers().iter() {
                let name_str = name.as_str();
                if name_str.eq_ignore_ascii_case("transfer-encoding")
                    || name_str.eq_ignore_ascii_case("content-length")
                    || name_str.eq_ignore_ascii_case("connection")
                    || name_str.eq_ignore_ascii_case("content-type")
                {
                    continue;
                }
                if let Ok(header) = Header::from_bytes(name_str.as_bytes(), value.as_bytes()) {
                    headers.push(header);
                }
            }
            if let Some(trace_id) = trace_id {
                push_trace_id_header(&mut headers, trace_id);
            }
            let upstream_content_type = upstream
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_string());

            if response_adapter == ResponseAdapter::AnthropicSse
                && (is_stream
                    || upstream_content_type
                        .as_deref()
                        .map(|value| value.to_ascii_lowercase().starts_with("text/event-stream"))
                        .unwrap_or(false))
            {
                if let Ok(content_type_header) =
                    Header::from_bytes(b"Content-Type".as_slice(), b"text/event-stream".as_slice())
                {
                    headers.push(content_type_header);
                }
                let usage_collector = Arc::new(Mutex::new(UpstreamResponseUsage::default()));
                let response = Response::new(
                    status,
                    headers,
                    AnthropicSseReader::new(upstream, Arc::clone(&usage_collector)),
                    None,
                    None,
                );
                let delivery_error = request.respond(response).err().map(|err| err.to_string());
                let usage = usage_collector
                    .lock()
                    .map(|guard| guard.clone())
                    .unwrap_or_default();
                return Ok(UpstreamResponseBridgeResult {
                    usage,
                    stream_terminal_seen: true,
                    stream_terminal_error: None,
                    delivery_error,
                    upstream_error_hint: None,
                    bridge_error: None,
                });
            }

            let upstream_body = upstream
                .bytes()
                .map_err(|err| format!("read upstream body failed: {err}"))?;
            let usage = serde_json::from_slice::<Value>(upstream_body.as_ref())
                .ok()
                .map(|value| parse_usage_from_json(&value))
                .unwrap_or_default();

            let (body, content_type) = match adapt_upstream_response(
                response_adapter,
                upstream_content_type.as_deref(),
                upstream_body.as_ref(),
            ) {
                Ok(result) => result,
                Err(err) => (
                    build_anthropic_error_body(&format!("response conversion failed: {err}")),
                    "application/json",
                ),
            };
            if let Ok(content_type_header) =
                Header::from_bytes(b"Content-Type".as_slice(), content_type.as_bytes())
            {
                headers.push(content_type_header);
            }

            let len = Some(body.len());
            let response = Response::new(status, headers, std::io::Cursor::new(body), len, None);
            let delivery_error = request.respond(response).err().map(|err| err.to_string());
            let upstream_error_hint =
                extract_error_hint_from_body(status.0, upstream_body.as_ref());
            Ok(UpstreamResponseBridgeResult {
                usage,
                stream_terminal_seen: true,
                stream_terminal_error: None,
                delivery_error,
                upstream_error_hint,
                bridge_error: None,
            })
        }
    }
}

fn resolve_stream_keepalive_frame(
    response_adapter: ResponseAdapter,
    request_path: &str,
) -> SseKeepAliveFrame {
    match response_adapter {
        ResponseAdapter::Passthrough => {
            if request_path.starts_with("/v1/responses") {
                SseKeepAliveFrame::OpenAIResponses
            } else {
                SseKeepAliveFrame::Comment
            }
        }
        ResponseAdapter::OpenAIChatCompletionsSse => SseKeepAliveFrame::OpenAIChatCompletions,
        ResponseAdapter::OpenAICompletionsSse => SseKeepAliveFrame::OpenAICompletions,
        ResponseAdapter::AnthropicSse => SseKeepAliveFrame::Anthropic,
        ResponseAdapter::OpenAIChatCompletionsJson
        | ResponseAdapter::OpenAICompletionsJson
        | ResponseAdapter::AnthropicJson => SseKeepAliveFrame::Comment,
    }
}
