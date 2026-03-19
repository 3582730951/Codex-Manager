use tiny_http::Request;

use super::super::super::request_log::RequestLogUsage;
use super::execution_context::GatewayUpstreamExecutionContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamBridgeFailure {
    ExplicitUpstreamError,
    Timeout,
    Disconnected,
    IncompleteBeforeOutput,
    IncompleteAfterOutput,
    GatewayBridgeError,
}

impl StreamBridgeFailure {
    fn status_for_log(self, upstream_status_code: u16) -> u16 {
        match self {
            Self::Timeout => 504,
            Self::ExplicitUpstreamError
            | Self::Disconnected
            | Self::IncompleteBeforeOutput
            | Self::IncompleteAfterOutput => upstream_status_code,
            Self::GatewayBridgeError => 502,
        }
    }

    fn error_label(self) -> &'static str {
        match self {
            Self::ExplicitUpstreamError => "upstream_stream_terminal_error",
            Self::Timeout => "upstream_stream_idle_timeout",
            Self::Disconnected => "upstream_disconnect_before_terminal",
            Self::IncompleteBeforeOutput => "stream_incomplete_before_output",
            Self::IncompleteAfterOutput => "stream_incomplete_after_output",
            Self::GatewayBridgeError => "gateway_bridge_error",
        }
    }

    fn should_mark_network_failure(self) -> bool {
        matches!(self, Self::Timeout)
    }

    fn should_mark_local_incomplete_strike(self) -> bool {
        matches!(self, Self::IncompleteBeforeOutput)
    }
}

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

fn classify_stream_bridge_failure(
    stream_terminal_seen: bool,
    stream_terminal_error: Option<&str>,
    has_output_signal: bool,
) -> Option<StreamBridgeFailure> {
    let Some(message) = stream_terminal_error
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return (!stream_terminal_seen).then_some(if has_output_signal {
            StreamBridgeFailure::IncompleteAfterOutput
        } else {
            StreamBridgeFailure::IncompleteBeforeOutput
        });
    };
    if stream_terminal_seen {
        return Some(StreamBridgeFailure::ExplicitUpstreamError);
    }

    let normalized = message.to_ascii_lowercase();
    if normalized.contains("超时")
        || normalized.contains("timed out")
        || normalized.contains("timeout")
    {
        return Some(StreamBridgeFailure::Timeout);
    }
    if normalized.contains("连接中断")
        || normalized.contains("broken pipe")
        || normalized.contains("connection reset")
        || normalized.contains("connection aborted")
        || normalized.contains("forcibly closed")
    {
        return Some(StreamBridgeFailure::Disconnected);
    }
    if normalized.contains("中途中断") || normalized.contains("未正常结束") {
        return Some(if has_output_signal {
            StreamBridgeFailure::IncompleteAfterOutput
        } else {
            StreamBridgeFailure::IncompleteBeforeOutput
        });
    }
    Some(StreamBridgeFailure::GatewayBridgeError)
}

fn describe_stream_bridge_failure(
    failure: StreamBridgeFailure,
    bridge_error_message: Option<&str>,
) -> String {
    let detail = bridge_error_message
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match failure {
        StreamBridgeFailure::ExplicitUpstreamError => detail
            .map(|message| format!("upstream_stream_terminal_error: {message}"))
            .unwrap_or_else(|| failure.error_label().to_string()),
        _ => detail
            .map(|message| format!("{}: {message}", failure.error_label()))
            .unwrap_or_else(|| failure.error_label().to_string()),
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
    let bridge_has_output_signal =
        bridge_output_text_len > 0 || bridge.usage.output_tokens.unwrap_or_default() > 0;
    super::super::super::trace_log::log_bridge_result(
        trace_id,
        format!("{response_adapter:?}").as_str(),
        path,
        client_is_stream,
        bridge.stream_terminal_seen,
        bridge.stream_terminal_error.as_deref(),
        bridge.delivery_error.as_deref(),
        bridge.bridge_error.as_deref(),
        bridge_output_text_len,
        bridge.usage.output_tokens,
    );

    if let Some(upstream_hint) = bridge.upstream_error_hint.as_deref() {
        final_error = Some(upstream_hint.to_string());
    } else if status_code >= 400 {
        final_error = last_attempt_error.map(str::to_string);
    }

    let bridge_ok = bridge.is_ok(client_is_stream);
    let stream_failure = if client_is_stream {
        classify_stream_bridge_failure(
            bridge.stream_terminal_seen,
            bridge.stream_terminal_error.as_deref(),
            bridge_has_output_signal,
        )
    } else {
        None
    };
    let client_delivery_failed = bridge
        .delivery_error
        .as_deref()
        .is_some_and(is_client_disconnect_error);
    let bridge_error_message = bridge.error_message(client_is_stream);
    if final_error.is_none() {
        if let Some(bridge_error) = bridge.bridge_error.as_deref() {
            final_error = Some(bridge_error.to_string());
        } else if client_delivery_failed {
            final_error = Some("client_cancelled".to_string());
        } else if let Some(stream_failure) = stream_failure {
            final_error = Some(describe_stream_bridge_failure(
                stream_failure,
                bridge_error_message.as_deref(),
            ));
        } else if bridge.delivery_error.is_some() {
            final_error = Some("gateway_bridge_error".to_string());
        } else if !bridge_ok {
            final_error = Some(
                bridge_error_message.unwrap_or_else(|| "gateway_bridge_error".to_string()),
            );
        }
    }
    let status_for_log = if status_code >= 400 {
        status_code
    } else if client_delivery_failed {
        499
    } else if let Some(stream_failure) = stream_failure {
        stream_failure.status_for_log(status_code)
    } else if bridge_ok {
        status_code
    } else {
        502
    };

    if stream_failure.is_some_and(StreamBridgeFailure::should_mark_network_failure) {
        super::super::super::mark_account_cooldown(
            account_id,
            super::super::super::CooldownReason::Network,
        );
        super::super::super::record_route_quality(account_id, 502);
    }
    if stream_failure.is_some_and(StreamBridgeFailure::should_mark_local_incomplete_strike)
        && super::super::super::local_burn::record_stream_incomplete_unknown(account_id)
    {
        super::super::super::mark_account_cooldown(
            account_id,
            super::super::super::CooldownReason::Network,
        );
        super::super::super::record_route_quality(account_id, 502);
    }

    let usage = bridge.usage;
    let usage_for_log = RequestLogUsage {
        input_tokens: usage.input_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.total_tokens,
        reasoning_output_tokens: usage.reasoning_output_tokens,
    };
    super::super::super::local_burn::record_request_usage(account_id, usage_for_log);
    context.log_final_result_with_model(
        Some(account_id),
        last_attempt_url,
        model_for_log,
        status_for_log,
        usage_for_log,
        final_error.as_deref(),
        started_at.elapsed().as_millis(),
        attempted_account_ids,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        classify_stream_bridge_failure, describe_stream_bridge_failure, StreamBridgeFailure,
    };

    #[test]
    fn stream_bridge_failure_treats_seen_terminal_errors_as_explicit_failures() {
        assert_eq!(
            classify_stream_bridge_failure(true, Some("model overloaded"), false),
            Some(StreamBridgeFailure::ExplicitUpstreamError)
        );
    }

    #[test]
    fn stream_bridge_failure_maps_timeout_messages_to_504_class() {
        assert_eq!(
            classify_stream_bridge_failure(false, Some("上游请求超时"), false),
            Some(StreamBridgeFailure::Timeout)
        );
        assert_eq!(StreamBridgeFailure::Timeout.status_for_log(200), 504);
    }

    #[test]
    fn stream_bridge_failure_maps_disconnect_and_incomplete_separately() {
        assert_eq!(
            classify_stream_bridge_failure(false, Some("上游流读取失败（连接中断）"), false),
            Some(StreamBridgeFailure::Disconnected)
        );
        assert_eq!(
            classify_stream_bridge_failure(false, Some("上游流中途中断（未正常结束）"), false),
            Some(StreamBridgeFailure::IncompleteBeforeOutput)
        );
        assert_eq!(
            classify_stream_bridge_failure(false, None, false),
            Some(StreamBridgeFailure::IncompleteBeforeOutput)
        );
        assert_eq!(
            classify_stream_bridge_failure(false, None, true),
            Some(StreamBridgeFailure::IncompleteAfterOutput)
        );
    }

    #[test]
    fn upstream_stream_failures_keep_original_http_status_in_request_log() {
        assert_eq!(StreamBridgeFailure::ExplicitUpstreamError.status_for_log(200), 200);
        assert_eq!(StreamBridgeFailure::Disconnected.status_for_log(200), 200);
        assert_eq!(StreamBridgeFailure::IncompleteBeforeOutput.status_for_log(200), 200);
        assert_eq!(StreamBridgeFailure::IncompleteAfterOutput.status_for_log(200), 200);
        assert_eq!(StreamBridgeFailure::GatewayBridgeError.status_for_log(200), 502);
    }

    #[test]
    fn explicit_upstream_stream_failures_preserve_terminal_error_detail() {
        assert_eq!(
            describe_stream_bridge_failure(
                StreamBridgeFailure::ExplicitUpstreamError,
                Some("code=server_error request failed")
            ),
            "upstream_stream_terminal_error: code=server_error request failed"
        );
        assert_eq!(
            describe_stream_bridge_failure(
                StreamBridgeFailure::GatewayBridgeError,
                Some("content_type_mismatch")
            ),
            "gateway_bridge_error: content_type_mismatch"
        );
    }

    #[test]
    fn only_disconnect_like_failures_trigger_network_penalty() {
        assert!(!StreamBridgeFailure::Disconnected.should_mark_network_failure());
        assert!(!StreamBridgeFailure::GatewayBridgeError.should_mark_network_failure());
        assert!(StreamBridgeFailure::Timeout.should_mark_network_failure());
        assert!(!StreamBridgeFailure::ExplicitUpstreamError.should_mark_network_failure());
        assert!(!StreamBridgeFailure::IncompleteBeforeOutput.should_mark_network_failure());
        assert!(!StreamBridgeFailure::IncompleteAfterOutput.should_mark_network_failure());
    }
}
