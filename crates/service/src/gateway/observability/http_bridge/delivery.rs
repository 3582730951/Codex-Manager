use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Request, Response, StatusCode};

use super::super::{
    adapt_upstream_response, adapt_upstream_response_with_tool_name_restore_map,
    build_anthropic_error_body, ResponseAdapter, ToolNameRestoreMap,
};
use super::{
    collect_non_stream_json_from_sse_bytes, extract_error_hint_from_body,
    extract_error_message_from_json, looks_like_sse_payload, merge_usage, parse_usage_from_json,
    push_trace_id_header, usage_has_signal, AnthropicSseReader, OpenAIChatCompletionsSseReader,
    OpenAICompletionsSseReader, PassthroughSseCollector, PassthroughSseUsageReader,
    SseKeepAliveFrame, UpstreamResponseBridgeResult, UpstreamResponseUsage,
};

const REQUEST_ID_HEADER_CANDIDATES: &[&str] = &["x-request-id", "x-oai-request-id"];
const CF_RAY_HEADER_NAME: &str = "cf-ray";
const AUTH_ERROR_HEADER_NAME: &str = "x-openai-authorization-error";

fn is_compact_request_path(path: &str) -> bool {
    path == "/v1/responses/compact" || path.starts_with("/v1/responses/compact?")
}

fn first_upstream_header(headers: &reqwest::header::HeaderMap, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        headers
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn compact_debug_suffix(
    kind: Option<&str>,
    request_id: Option<&str>,
    cf_ray: Option<&str>,
    auth_error: Option<&str>,
    identity_error_code: Option<&str>,
) -> String {
    let mut details = Vec::new();
    if let Some(kind) = kind.map(str::trim).filter(|value| !value.is_empty()) {
        details.push(format!("kind={kind}"));
    }
    if let Some(request_id) = request_id.map(str::trim).filter(|value| !value.is_empty()) {
        details.push(format!("request_id={request_id}"));
    }
    if let Some(cf_ray) = cf_ray.map(str::trim).filter(|value| !value.is_empty()) {
        details.push(format!("cf_ray={cf_ray}"));
    }
    if let Some(auth_error) = auth_error.map(str::trim).filter(|value| !value.is_empty()) {
        details.push(format!("auth_error={auth_error}"));
    }
    if let Some(identity_error_code) = identity_error_code
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        details.push(format!("identity_error_code={identity_error_code}"));
    }
    if details.is_empty() {
        String::new()
    } else {
        format!(" [{}]", details.join(", "))
    }
}

fn with_upstream_debug_suffix(
    message: Option<String>,
    kind: Option<&str>,
    request_id: Option<&str>,
    cf_ray: Option<&str>,
    auth_error: Option<&str>,
    identity_error_code: Option<&str>,
) -> Option<String> {
    let message = message?;
    let suffix = compact_debug_suffix(kind, request_id, cf_ray, auth_error, identity_error_code);
    if suffix.is_empty() {
        Some(message)
    } else {
        Some(format!("{message}{suffix}"))
    }
}

fn looks_like_blocked_marker(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    normalized.contains("blocked")
        || normalized.contains("unsupported_country_region_territory")
        || normalized.contains("unsupported_country")
        || normalized.contains("region_restricted")
}

fn classify_compact_invalid_success_kind(
    body: &[u8],
    cf_ray: Option<&str>,
    auth_error: Option<&str>,
    identity_error_code: Option<&str>,
) -> &'static str {
    if auth_error.is_some_and(looks_like_blocked_marker)
        || identity_error_code.is_some_and(looks_like_blocked_marker)
    {
        return "cloudflare_blocked";
    }
    if let Some(hint) = extract_error_hint_from_body(502, body) {
        if hint.contains("Cloudflare") {
            return "cloudflare_challenge";
        }
        if hint.contains("HTML 错误页") {
            return "html";
        }
    }
    if identity_error_code.is_some() {
        return "identity_error";
    }
    if auth_error.is_some() {
        return "auth_error";
    }
    if cf_ray.is_some() {
        return "cloudflare_edge";
    }
    if serde_json::from_slice::<Value>(body).is_ok() {
        "invalid_success_body"
    } else if body.is_empty() {
        "empty"
    } else {
        "non_json"
    }
}

fn classify_compact_non_success_kind(
    status_code: u16,
    content_type: Option<&str>,
    body: &[u8],
    cf_ray: Option<&str>,
    auth_error: Option<&str>,
    identity_error_code: Option<&str>,
) -> &'static str {
    if auth_error.is_some_and(looks_like_blocked_marker)
        || identity_error_code.is_some_and(looks_like_blocked_marker)
    {
        return "cloudflare_blocked";
    }
    if let Some(hint) = extract_error_hint_from_body(status_code, body) {
        if hint.contains("Cloudflare") {
            return "cloudflare_challenge";
        }
        if hint.contains("HTML 错误页") {
            return "html";
        }
    }
    if content_type
        .map(crate::gateway::is_html_content_type)
        .unwrap_or(false)
    {
        return "html";
    }
    if identity_error_code.is_some() {
        return "identity_error";
    }
    if auth_error.is_some() {
        return "auth_error";
    }
    if cf_ray.is_some() {
        return "cloudflare_edge";
    }
    if serde_json::from_slice::<Value>(body).is_ok() {
        "json_error"
    } else if body.is_empty() {
        "empty"
    } else {
        "non_json"
    }
}

fn compact_success_body_is_valid(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("output").cloned())
        .is_some_and(|output| output.is_array())
}

fn build_invalid_compact_success_message(
    body: &[u8],
    request_id: Option<&str>,
    cf_ray: Option<&str>,
    auth_error: Option<&str>,
    identity_error_code: Option<&str>,
) -> String {
    let kind = classify_compact_invalid_success_kind(body, cf_ray, auth_error, identity_error_code);
    if let Ok(value) = serde_json::from_slice::<Value>(body) {
        if let Some(message) = extract_error_message_from_json(&value) {
            return format!(
                "上游 compact 响应格式异常：{message}{}",
                compact_debug_suffix(
                    Some(kind),
                    request_id,
                    cf_ray,
                    auth_error,
                    identity_error_code
                )
            );
        }
    }
    if let Some(hint) = extract_error_hint_from_body(502, body) {
        return format!(
            "上游 compact 响应格式异常：{hint}{}",
            compact_debug_suffix(
                Some(kind),
                request_id,
                cf_ray,
                auth_error,
                identity_error_code
            )
        );
    }
    format!(
        "上游 compact 响应格式异常（未返回 output 数组）{}",
        compact_debug_suffix(
            Some(kind),
            request_id,
            cf_ray,
            auth_error,
            identity_error_code
        )
    )
}

fn compact_non_success_body_should_be_normalized(
    status_code: u16,
    content_type: Option<&str>,
    body: &[u8],
    auth_error: Option<&str>,
    identity_error_code: Option<&str>,
) -> bool {
    if status_code < 400 {
        return false;
    }
    if auth_error
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
        || identity_error_code
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
    {
        return true;
    }
    if content_type
        .map(crate::gateway::is_html_content_type)
        .unwrap_or(false)
    {
        return true;
    }
    extract_error_hint_from_body(status_code, body)
        .is_some_and(|hint| hint.contains("Cloudflare") || hint.contains("HTML 错误页"))
}

fn build_compact_non_success_message(
    status_code: u16,
    content_type: Option<&str>,
    body: &[u8],
    request_id: Option<&str>,
    cf_ray: Option<&str>,
    auth_error: Option<&str>,
    identity_error_code: Option<&str>,
) -> String {
    let kind = classify_compact_non_success_kind(
        status_code,
        content_type,
        body,
        cf_ray,
        auth_error,
        identity_error_code,
    );
    if let Ok(value) = serde_json::from_slice::<Value>(body) {
        if let Some(message) = extract_error_message_from_json(&value) {
            return format!(
                "上游 compact 请求失败：{message}{}",
                compact_debug_suffix(
                    Some(kind),
                    request_id,
                    cf_ray,
                    auth_error,
                    identity_error_code
                )
            );
        }
    }
    if let Some(hint) = extract_error_hint_from_body(status_code, body) {
        return format!(
            "上游 compact 请求失败：{hint}{}",
            compact_debug_suffix(
                Some(kind),
                request_id,
                cf_ray,
                auth_error,
                identity_error_code
            )
        );
    }
    format!(
        "上游 compact 请求失败：status={status_code}{}",
        compact_debug_suffix(
            Some(kind),
            request_id,
            cf_ray,
            auth_error,
            identity_error_code
        )
    )
}

fn respond_synthesized_compact_error_body(
    request: Request,
    status_code: u16,
    usage: UpstreamResponseUsage,
    message: String,
    request_id: Option<&str>,
    cf_ray: Option<&str>,
    trace_id: Option<&str>,
) -> UpstreamResponseBridgeResult {
    let response = crate::gateway::error_response::terminal_text_response(
        status_code,
        message.as_str(),
        trace_id,
    );
    let delivery_error = request.respond(response).err().map(|err| err.to_string());
    UpstreamResponseBridgeResult {
        usage,
        stream_terminal_seen: true,
        stream_terminal_error: None,
        delivery_error,
        upstream_error_hint: Some(message),
        delivered_status_code: Some(status_code),
        upstream_request_id: request_id.map(str::to_string),
        upstream_cf_ray: cf_ray.map(str::to_string),
        upstream_content_type: Some("application/json".to_string()),
    }
}

fn respond_invalid_compact_success_body(
    request: Request,
    usage: UpstreamResponseUsage,
    body: &[u8],
    request_id: Option<&str>,
    cf_ray: Option<&str>,
    auth_error: Option<&str>,
    identity_error_code: Option<&str>,
    trace_id: Option<&str>,
) -> UpstreamResponseBridgeResult {
    respond_synthesized_compact_error_body(
        request,
        502,
        usage,
        build_invalid_compact_success_message(
            body,
            request_id,
            cf_ray,
            auth_error,
            identity_error_code,
        ),
        request_id,
        cf_ray,
        trace_id,
    )
}

fn respond_invalid_compact_non_success_body(
    request: Request,
    status_code: u16,
    usage: UpstreamResponseUsage,
    body: &[u8],
    content_type: Option<&str>,
    request_id: Option<&str>,
    cf_ray: Option<&str>,
    auth_error: Option<&str>,
    identity_error_code: Option<&str>,
    trace_id: Option<&str>,
) -> UpstreamResponseBridgeResult {
    respond_synthesized_compact_error_body(
        request,
        status_code,
        usage,
        build_compact_non_success_message(
            status_code,
            content_type,
            body,
            request_id,
            cf_ray,
            auth_error,
            identity_error_code,
        ),
        request_id,
        cf_ray,
        trace_id,
    )
}

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
    let upstream_request_id =
        first_upstream_header(upstream.headers(), REQUEST_ID_HEADER_CANDIDATES);
    let upstream_cf_ray = first_upstream_header(upstream.headers(), &[CF_RAY_HEADER_NAME]);
    let upstream_auth_error = first_upstream_header(upstream.headers(), &[AUTH_ERROR_HEADER_NAME]);
    let upstream_identity_error_code =
        crate::gateway::extract_identity_error_code_from_headers(upstream.headers());
    let upstream_content_type = upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_string());
    match response_adapter {
        ResponseAdapter::Passthrough => {
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
                let is_compact_request = is_compact_request_path(request_path);
                if detected_sse {
                    let (synthesized_body, mut usage) =
                        collect_non_stream_json_from_sse_bytes(upstream_body.as_ref());
                    let synthesized_response = synthesized_body.is_some();
                    let body = synthesized_body.unwrap_or_else(|| upstream_body.to_vec());
                    if let Ok(value) = serde_json::from_slice::<Value>(&body) {
                        merge_usage(&mut usage, parse_usage_from_json(&value));
                    }
                    let upstream_error_hint = with_upstream_debug_suffix(
                        extract_error_hint_from_body(status.0, &body),
                        None,
                        upstream_request_id.as_deref(),
                        upstream_cf_ray.as_deref(),
                        upstream_auth_error.as_deref(),
                        upstream_identity_error_code.as_deref(),
                    );
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
                    if status.0 < 400
                        && is_compact_request
                        && !compact_success_body_is_valid(body.as_ref())
                    {
                        return Ok(respond_invalid_compact_success_body(
                            request,
                            usage,
                            body.as_ref(),
                            upstream_request_id.as_deref(),
                            upstream_cf_ray.as_deref(),
                            upstream_auth_error.as_deref(),
                            upstream_identity_error_code.as_deref(),
                            trace_id,
                        ));
                    }
                    if is_compact_request
                        && compact_non_success_body_should_be_normalized(
                            status.0,
                            upstream_content_type.as_deref(),
                            body.as_ref(),
                            upstream_auth_error.as_deref(),
                            upstream_identity_error_code.as_deref(),
                        )
                    {
                        return Ok(respond_invalid_compact_non_success_body(
                            request,
                            status.0,
                            usage,
                            body.as_ref(),
                            upstream_content_type.as_deref(),
                            upstream_request_id.as_deref(),
                            upstream_cf_ray.as_deref(),
                            upstream_auth_error.as_deref(),
                            upstream_identity_error_code.as_deref(),
                            trace_id,
                        ));
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
                        delivered_status_code: None,
                        upstream_request_id: upstream_request_id.clone(),
                        upstream_cf_ray: upstream_cf_ray.clone(),
                        upstream_content_type: upstream_content_type.clone(),
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
                if status.0 < 400
                    && is_compact_request
                    && !compact_success_body_is_valid(upstream_body.as_ref())
                {
                    return Ok(respond_invalid_compact_success_body(
                        request,
                        usage,
                        upstream_body.as_ref(),
                        upstream_request_id.as_deref(),
                        upstream_cf_ray.as_deref(),
                        upstream_auth_error.as_deref(),
                        upstream_identity_error_code.as_deref(),
                        trace_id,
                    ));
                }
                if is_compact_request
                    && compact_non_success_body_should_be_normalized(
                        status.0,
                        upstream_content_type.as_deref(),
                        upstream_body.as_ref(),
                        upstream_auth_error.as_deref(),
                        upstream_identity_error_code.as_deref(),
                    )
                {
                    return Ok(respond_invalid_compact_non_success_body(
                        request,
                        status.0,
                        usage,
                        upstream_body.as_ref(),
                        upstream_content_type.as_deref(),
                        upstream_request_id.as_deref(),
                        upstream_cf_ray.as_deref(),
                        upstream_auth_error.as_deref(),
                        upstream_identity_error_code.as_deref(),
                        trace_id,
                    ));
                }
                let upstream_error_hint = with_upstream_debug_suffix(
                    extract_error_hint_from_body(status.0, upstream_body.as_ref()),
                    None,
                    upstream_request_id.as_deref(),
                    upstream_cf_ray.as_deref(),
                    upstream_auth_error.as_deref(),
                    upstream_identity_error_code.as_deref(),
                );
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
                    delivered_status_code: None,
                    upstream_request_id: upstream_request_id.clone(),
                    upstream_cf_ray: upstream_cf_ray.clone(),
                    upstream_content_type: upstream_content_type.clone(),
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
                let upstream_error_hint = with_upstream_debug_suffix(
                    extract_error_hint_from_body(status.0, upstream_body.as_ref()),
                    None,
                    upstream_request_id.as_deref(),
                    upstream_cf_ray.as_deref(),
                    upstream_auth_error.as_deref(),
                    upstream_identity_error_code.as_deref(),
                );
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
                    delivered_status_code: None,
                    upstream_request_id: upstream_request_id.clone(),
                    upstream_cf_ray: upstream_cf_ray.clone(),
                    upstream_content_type: upstream_content_type.clone(),
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
                    upstream_error_hint: with_upstream_debug_suffix(
                        collector.upstream_error_hint,
                        None,
                        upstream_request_id.as_deref(),
                        upstream_cf_ray.as_deref(),
                        upstream_auth_error.as_deref(),
                        upstream_identity_error_code.as_deref(),
                    ),
                    delivered_status_code: None,
                    upstream_request_id: upstream_request_id.clone(),
                    upstream_cf_ray: upstream_cf_ray.clone(),
                    upstream_content_type: upstream_content_type.clone(),
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
                delivered_status_code: None,
                upstream_request_id,
                upstream_cf_ray,
                upstream_content_type,
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
                            ),
                            None,
                            None,
                        );
                        request.respond(response).err().map(|err| err.to_string())
                    } else {
                        let response = Response::new(
                            status,
                            headers,
                            OpenAICompletionsSseReader::new(upstream, Arc::clone(&usage_collector)),
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
                    delivered_status_code: None,
                    upstream_request_id: upstream_request_id.clone(),
                    upstream_cf_ray: upstream_cf_ray.clone(),
                    upstream_content_type: upstream_content_type.clone(),
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
            let (mut body, mut content_type) =
                match adapt_upstream_response_with_tool_name_restore_map(
                    response_adapter,
                    upstream_content_type.as_deref(),
                    upstream_body.as_ref(),
                    tool_name_restore_map,
                ) {
                    Ok(result) => result,
                    Err(err) => (
                        serde_json::to_vec(&json!({
                            "error": {
                                "message": format!("response conversion failed: {err}"),
                                "type": "server_error"
                            }
                        }))
                        .unwrap_or_else(|_| {
                            b"{\"error\":{\"message\":\"response conversion failed\",\"type\":\"server_error\"}}"
                                .to_vec()
                        }),
                        "application/json",
                    ),
                };
            if use_openai_sse_adapter
                && is_stream
                && status.0 < 400
                && !content_type.eq_ignore_ascii_case("text/event-stream")
            {
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
                        upstream_content_type
                            .as_deref()
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or("-")
                    );
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
            let upstream_error_hint = with_upstream_debug_suffix(
                extract_error_hint_from_body(status.0, upstream_body.as_ref()),
                None,
                upstream_request_id.as_deref(),
                upstream_cf_ray.as_deref(),
                upstream_auth_error.as_deref(),
                upstream_identity_error_code.as_deref(),
            );
            Ok(UpstreamResponseBridgeResult {
                usage,
                stream_terminal_seen: true,
                stream_terminal_error: None,
                delivery_error,
                upstream_error_hint,
                delivered_status_code: None,
                upstream_request_id,
                upstream_cf_ray,
                upstream_content_type,
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
                    delivered_status_code: None,
                    upstream_request_id: upstream_request_id.clone(),
                    upstream_cf_ray: upstream_cf_ray.clone(),
                    upstream_content_type: upstream_content_type.clone(),
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
            let upstream_error_hint = with_upstream_debug_suffix(
                extract_error_hint_from_body(status.0, upstream_body.as_ref()),
                None,
                upstream_request_id.as_deref(),
                upstream_cf_ray.as_deref(),
                upstream_auth_error.as_deref(),
                upstream_identity_error_code.as_deref(),
            );
            Ok(UpstreamResponseBridgeResult {
                usage,
                stream_terminal_seen: true,
                stream_terminal_error: None,
                delivery_error,
                upstream_error_hint,
                delivered_status_code: None,
                upstream_request_id,
                upstream_cf_ray,
                upstream_content_type,
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

#[cfg(test)]
mod tests {
    use super::{classify_compact_non_success_kind, compact_non_success_body_should_be_normalized};

    #[test]
    fn compact_header_only_identity_error_is_normalized_and_classified() {
        assert!(compact_non_success_body_should_be_normalized(
            403,
            Some("text/plain"),
            b"",
            None,
            Some("org_membership_required"),
        ));
        assert_eq!(
            classify_compact_non_success_kind(
                403,
                Some("text/plain"),
                b"",
                None,
                None,
                Some("org_membership_required"),
            ),
            "identity_error"
        );
    }

    #[test]
    fn compact_header_only_cf_ray_is_classified_as_cloudflare_edge() {
        assert_eq!(
            classify_compact_non_success_kind(
                502,
                Some("text/plain"),
                b"",
                Some("ray_compact_edge"),
                None,
                None,
            ),
            "cloudflare_edge"
        );
    }
}
