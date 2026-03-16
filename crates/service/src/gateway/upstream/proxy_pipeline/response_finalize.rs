use tiny_http::Request;

use super::super::super::request_log::RequestLogUsage;
use super::execution_context::GatewayUpstreamExecutionContext;

pub(in super::super) fn respond_terminal(
    request: Request,
    status_code: u16,
    message: String,
    trace_id: Option<&str>,
) -> Result<(), String> {
    let response =
        super::super::super::error_response::terminal_text_response(status_code, message, trace_id);
    let _ = request.respond(response);
    Ok(())
}

fn is_client_disconnect_error(message: &str) -> bool {
    let normalized = message.trim().to_ascii_lowercase();
    normalized.contains("broken pipe")
        || normalized.contains("connection reset")
        || normalized.contains("connection aborted")
        || normalized.contains("connection was forcibly closed")
        || normalized.contains("os error 32")
        || normalized.contains("os error 54")
        || normalized.contains("os error 104")
}

fn classify_upstream_hint(hint: &str) -> &'static str {
    let normalized = hint.trim().to_ascii_lowercase();
    if normalized.contains("cloudflare")
        || normalized.contains("安全验证")
        || normalized.contains("challenge")
    {
        "challenge"
    } else if normalized.contains("html 错误页") || normalized.contains("<html") {
        "html_error"
    } else {
        "upstream_hint"
    }
}

fn classify_finalize_error(
    status_code: u16,
    upstream_hint: Option<&str>,
    client_is_stream: bool,
    stream_terminal_seen: bool,
    stream_terminal_error: Option<&str>,
    delivery_error: Option<&str>,
) -> Option<&'static str> {
    if let Some(error) = delivery_error {
        if is_client_disconnect_error(error) {
            return Some("client_disconnect");
        }
        return Some("delivery_error");
    }
    if stream_terminal_error.is_some() {
        return Some("stream_error");
    }
    if client_is_stream && !stream_terminal_seen {
        return Some("stream_interrupted");
    }
    if let Some(hint) = upstream_hint {
        return Some(classify_upstream_hint(hint));
    }
    match status_code {
        429 => Some("rate_limited"),
        500..=599 => Some("upstream_5xx"),
        400..=499 => Some("upstream_4xx"),
        _ => None,
    }
}

fn derived_status_for_bridge_error(error_class: Option<&str>) -> Option<u16> {
    match error_class {
        Some("client_disconnect") => Some(499),
        Some(_) => Some(502),
        None => None,
    }
}

pub(super) fn respond_total_timeout(
    request: Request,
    context: &GatewayUpstreamExecutionContext<'_>,
    trace_id: &str,
    started_at: std::time::Instant,
    model_for_log: Option<&str>,
    attempted_account_ids: Option<&[String]>,
) -> Result<(), String> {
    let message = "upstream total timeout exceeded".to_string();
    context.log_final_result_with_model(
        None,
        None,
        model_for_log,
        504,
        RequestLogUsage::default(),
        Some(message.as_str()),
        started_at.elapsed().as_millis(),
        attempted_account_ids,
    );
    respond_terminal(request, 504, message, Some(trace_id))
}

pub(super) fn finalize_terminal_candidate(
    request: Request,
    context: &GatewayUpstreamExecutionContext<'_>,
    account_id: &str,
    last_attempt_url: Option<&str>,
    status_code: u16,
    message: String,
    trace_id: &str,
    started_at: std::time::Instant,
    model_for_log: Option<&str>,
    attempted_account_ids: Option<&[String]>,
) -> Result<(), String> {
    context.log_final_result_with_model(
        Some(account_id),
        last_attempt_url,
        model_for_log,
        status_code,
        RequestLogUsage::default(),
        Some(message.as_str()),
        started_at.elapsed().as_millis(),
        attempted_account_ids,
    );
    respond_terminal(request, status_code, message, Some(trace_id))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn finalize_upstream_response(
    request: Request,
    response: reqwest::blocking::Response,
    inflight_guard: super::super::super::AccountInFlightGuard,
    context: &GatewayUpstreamExecutionContext<'_>,
    account_id: &str,
    last_attempt_url: Option<&str>,
    last_attempt_error: Option<&str>,
    response_adapter: super::super::super::ResponseAdapter,
    tool_name_restore_map: &super::super::super::ToolNameRestoreMap,
    client_is_stream: bool,
    path: &str,
    trace_id: &str,
    started_at: std::time::Instant,
    model_for_log: Option<&str>,
    attempted_account_ids: Option<&[String]>,
) -> Result<(), String> {
    let status_code = response.status().as_u16();
    let mut final_error = None;

    let bridge = super::super::super::respond_with_upstream(
        request,
        response,
        inflight_guard,
        response_adapter,
        path,
        Some(tool_name_restore_map),
        client_is_stream,
        Some(trace_id),
    )?;
    let bridge_output_text_len = bridge
        .usage
        .output_text
        .as_deref()
        .map(str::trim)
        .map(str::len)
        .unwrap_or(0);
    super::super::super::trace_log::log_bridge_result(
        trace_id,
        format!("{response_adapter:?}").as_str(),
        path,
        client_is_stream,
        bridge.stream_terminal_seen,
        bridge.stream_terminal_error.as_deref(),
        bridge.delivery_error.as_deref(),
        bridge_output_text_len,
        bridge.usage.output_tokens,
    );

    if let Some(upstream_hint) = bridge.upstream_error_hint.as_deref() {
        final_error = Some(upstream_hint.to_string());
    } else if status_code >= 400 {
        final_error = last_attempt_error.map(str::to_string);
    }

    let bridge_ok = bridge.is_ok(client_is_stream);
    if final_error.is_none() && !bridge_ok {
        final_error = Some(
            bridge
                .error_message(client_is_stream)
                .unwrap_or_else(|| "upstream response incomplete".to_string()),
        );
    }

    let error_class = classify_finalize_error(
        status_code,
        bridge.upstream_error_hint.as_deref(),
        client_is_stream,
        bridge.stream_terminal_seen,
        bridge.stream_terminal_error.as_deref(),
        bridge.delivery_error.as_deref(),
    );
    let upstream_stream_failed =
        client_is_stream && (!bridge.stream_terminal_seen || bridge.stream_terminal_error.is_some());
    let client_delivery_failed = bridge
        .delivery_error
        .as_deref()
        .is_some_and(is_client_disconnect_error);
    let status_for_log = if status_code >= 400 {
        status_code
    } else if client_delivery_failed {
        499
    } else if let Some(derived) = derived_status_for_bridge_error(error_class) {
        derived
    } else if bridge_ok {
        status_code
    } else {
        502
    };

    if upstream_stream_failed
        || matches!(error_class, Some("stream_interrupted" | "stream_error" | "html_error"))
    {
        let _ = super::super::super::clear_manual_preferred_account_if(account_id);
        super::super::super::mark_account_cooldown(
            account_id,
            super::super::super::CooldownReason::Network,
        );
        super::super::super::record_route_quality(account_id, 502);
    }
    if matches!(error_class, Some("challenge")) {
        let _ = super::super::super::clear_manual_preferred_account_if(account_id);
        super::super::super::mark_account_cooldown(
            account_id,
            super::super::super::CooldownReason::Challenge,
        );
        super::super::super::record_route_quality(account_id, 403);
    }
    if let Some(class) = error_class {
        log::warn!(
            "event=gateway_finalize_error_class trace_id={} account_id={} class={} upstream_status={} final_status={}",
            trace_id,
            account_id,
            class,
            status_code,
            status_for_log
        );
    }

    let usage = bridge.usage;
    context.log_final_result_with_model(
        Some(account_id),
        last_attempt_url,
        model_for_log,
        status_for_log,
        RequestLogUsage {
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
            reasoning_output_tokens: usage.reasoning_output_tokens,
        },
        final_error.as_deref(),
        started_at.elapsed().as_millis(),
        attempted_account_ids,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{classify_finalize_error, classify_upstream_hint, derived_status_for_bridge_error};

    #[test]
    fn classify_upstream_hint_detects_challenge_and_html() {
        assert_eq!(
            classify_upstream_hint("Cloudflare 安全验证页（title=Just a moment...）"),
            "challenge"
        );
        assert_eq!(classify_upstream_hint("上游返回 HTML 错误页"), "html_error");
    }

    #[test]
    fn classify_finalize_error_detects_hidden_bridge_failures() {
        assert_eq!(
            classify_finalize_error(200, Some("Cloudflare 安全验证页"), false, true, None, None),
            Some("challenge")
        );
        assert_eq!(
            classify_finalize_error(200, Some("上游返回 HTML 错误页"), false, true, None, None),
            Some("html_error")
        );
        assert_eq!(
            classify_finalize_error(200, None, true, false, None, None),
            Some("stream_interrupted")
        );
        assert_eq!(
            classify_finalize_error(
                200,
                None,
                false,
                true,
                None,
                Some("broken pipe")
            ),
            Some("client_disconnect")
        );
    }

    #[test]
    fn derived_status_for_bridge_error_maps_hidden_failures_to_502() {
        assert_eq!(derived_status_for_bridge_error(Some("challenge")), Some(502));
        assert_eq!(
            derived_status_for_bridge_error(Some("client_disconnect")),
            Some(499)
        );
        assert_eq!(derived_status_for_bridge_error(None), None);
    }
}
