use tiny_http::Request;

use super::super::super::request_log::RequestLogUsage;
use crate::gateway::affinity::AffinityRoutingResolution;
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
    let _ = context.mark_account_unavailable_for_gateway_error(account_id, &message);
    super::super::super::record_scheduler_feedback(
        account_id,
        super::super::super::scheduler::SchedulerFeedback {
            status_code,
            elapsed_ms: started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            network_error: status_code >= 500,
            stream_failed: false,
        },
    );
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
    request_body: &[u8],
    last_attempt_url: Option<&str>,
    last_attempt_error: Option<&str>,
    response_adapter: super::super::super::ResponseAdapter,
    tool_name_restore_map: &super::super::super::ToolNameRestoreMap,
    client_is_stream: bool,
    path: &str,
    affinity_resolution: Option<&AffinityRoutingResolution>,
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
        bridge.delivered_status_code,
        bridge.upstream_error_hint.as_deref(),
        bridge.upstream_request_id.as_deref(),
        bridge.upstream_cf_ray.as_deref(),
        bridge.upstream_auth_error.as_deref(),
        bridge.upstream_identity_error_code.as_deref(),
        bridge.upstream_content_type.as_deref(),
        bridge.last_sse_event_type.as_deref(),
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

    let upstream_stream_failed = client_is_stream
        && (!bridge.stream_terminal_seen || bridge.stream_terminal_error.is_some());
    let client_delivery_failed = bridge
        .delivery_error
        .as_deref()
        .is_some_and(is_client_disconnect_error);
    let status_for_log = if client_delivery_failed {
        499
    } else if let Some(delivered_status_code) = bridge.delivered_status_code {
        delivered_status_code
    } else if status_code >= 400 {
        status_code
    } else if upstream_stream_failed {
        502
    } else if bridge_ok {
        status_code
    } else {
        502
    };

    if upstream_stream_failed {
        super::super::super::mark_account_cooldown(
            account_id,
            super::super::super::CooldownReason::Network,
        );
        super::super::super::record_route_quality(account_id, 502);
    }

    if let Some(error) = final_error.as_deref() {
        let _ = context.mark_account_unavailable_for_gateway_error(account_id, error);
    }

    super::super::super::record_scheduler_feedback(
        account_id,
        super::super::super::scheduler::SchedulerFeedback {
            status_code: status_for_log,
            elapsed_ms: started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            network_error: upstream_stream_failed,
            stream_failed: upstream_stream_failed,
        },
    );

    let usage = bridge.usage;
    if affinity_resolution.is_some() {
        super::super::super::affinity::record_affinity_attempt_feedback(
            account_id,
            status_for_log,
            final_error.as_deref(),
        );
    }
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
    if let Some(resolution) = affinity_resolution {
        if bridge_ok && status_for_log < 400 {
            let response_adapter_label = format!("{response_adapter:?}");
            if let Some(completed_response_body) = bridge.completed_response_body.as_deref() {
                if let Err(err) = super::super::super::affinity::finalize_affinity_success(
                    context.storage(),
                    resolution,
                    context.platform_key_hash(),
                    account_id,
                    request_body,
                    Some(completed_response_body),
                    response_adapter_label.as_str(),
                    context.protocol_type(),
                ) {
                    log::warn!(
                        "event=gateway_affinity_finalize_failed trace_id={} account_id={} err={}",
                        trace_id,
                        account_id,
                        err
                    );
                }
            } else {
                log::warn!(
                    "event=gateway_affinity_finalize_skipped trace_id={} account_id={} reason=missing_completed_response_body",
                    trace_id,
                    account_id
                );
            }
        }
    }
    Ok(())
}
