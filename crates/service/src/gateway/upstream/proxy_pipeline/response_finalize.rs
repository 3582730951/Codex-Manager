use codexmanager_core::storage::Account;
use tiny_http::Request;

use super::super::super::request_log::RequestLogUsage;
use super::execution_context::GatewayUpstreamExecutionContext;
use crate::gateway::affinity::AffinityRoutingResolution;
use crate::gateway::http_bridge::UpstreamCompletionState;

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

fn mark_account_unavailable_from_bridge_signals(
    context: &GatewayUpstreamExecutionContext<'_>,
    account_id: &str,
    final_error: Option<&str>,
    upstream_auth_error: Option<&str>,
    upstream_identity_error_code: Option<&str>,
) {
    for signal in [
        final_error,
        upstream_auth_error,
        upstream_identity_error_code,
    ] {
        let Some(signal) = signal.map(str::trim).filter(|value| !value.is_empty()) else {
            continue;
        };
        if crate::account_status::mark_account_unavailable_for_auth_error(
            context.storage(),
            account_id,
            signal,
        ) {
            return;
        }
        if crate::account_status::mark_account_unavailable_for_usage_http_error(
            context.storage(),
            account_id,
            signal,
        ) {
            return;
        }
        if crate::account_status::mark_account_unavailable_for_deactivation_error(
            context.storage(),
            account_id,
            signal,
        ) {
            return;
        }
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
    account: &Account,
    request_body: &crate::gateway::RequestPayload,
    last_attempt_url: Option<&str>,
    last_attempt_error: Option<&str>,
    response_adapter: super::super::super::ResponseAdapter,
    tool_name_restore_map: &super::super::super::ToolNameRestoreMap,
    client_is_stream: bool,
    path: &str,
    affinity_resolution: Option<&AffinityRoutingResolution>,
    peer_runtime_key: Option<&str>,
    trace_id: &str,
    started_at: std::time::Instant,
    model_for_log: Option<&str>,
    attempted_account_ids: Option<&[String]>,
) -> Result<(), String> {
    let account_id = account.id.as_str();
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
        bridge.upstream_completion_state,
        bridge.upstream_completion_error.as_deref(),
        bridge.delivery_state,
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

    let upstream_reader_failed = client_is_stream && bridge.is_reader_error();
    let upstream_eof_without_terminal = client_is_stream
        && bridge.upstream_completion_state == UpstreamCompletionState::EofWithoutTerminal;
    let client_delivery_failed = bridge.is_client_disconnect()
        || bridge
            .delivery_error
            .as_deref()
            .is_some_and(is_client_disconnect_error);
    let status_for_log = if client_delivery_failed {
        499
    } else if let Some(delivered_status_code) = bridge.delivered_status_code {
        delivered_status_code
    } else if status_code >= 400 {
        status_code
    } else if upstream_reader_failed || upstream_eof_without_terminal {
        502
    } else if bridge_ok {
        status_code
    } else {
        502
    };

    if upstream_reader_failed {
        super::super::super::mark_account_cooldown(
            account_id,
            super::super::super::CooldownReason::Network,
        );
        super::super::super::record_route_quality(account_id, 502);
    }

    mark_account_unavailable_from_bridge_signals(
        context,
        account_id,
        final_error.as_deref(),
        bridge.upstream_auth_error.as_deref(),
        bridge.upstream_identity_error_code.as_deref(),
    );

    super::super::super::record_scheduler_feedback(
        account_id,
        super::super::super::scheduler::SchedulerFeedback {
            status_code: status_for_log,
            elapsed_ms: started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            network_error: upstream_reader_failed,
            stream_failed: upstream_reader_failed,
        },
    );

    let hard_quota_headers = {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(retry_after) = bridge
            .upstream_retry_after
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if let Ok(value) = reqwest::header::HeaderValue::from_str(retry_after) {
                headers.insert(reqwest::header::RETRY_AFTER, value);
            }
        }
        if let Some(date) = bridge
            .upstream_date
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if let Ok(value) = reqwest::header::HeaderValue::from_str(date) {
                headers.insert(reqwest::header::DATE, value);
            }
        }
        (!headers.is_empty()).then_some(headers)
    };
    let usage = bridge.usage;
    if affinity_resolution.is_some() && !client_delivery_failed && !upstream_eof_without_terminal {
        super::super::super::affinity::record_affinity_attempt_feedback(
            account_id,
            status_for_log,
            final_error.as_deref(),
        );
    }
    if bridge_ok
        && status_for_log < 400
        && !client_delivery_failed
        && !upstream_eof_without_terminal
    {
        super::super::super::record_account_group_proxy_success(account);
        super::super::super::affinity::clear_account_hard_quota_exhaustion(
            context.storage(),
            account_id,
        );
        if let Some(runtime_key) = peer_runtime_key {
            super::super::super::affinity::record_peer_runtime_success(runtime_key, account_id);
        }
    } else {
        if final_error
            .as_deref()
            .map(crate::error_codes::classify_message)
            == Some(crate::error_codes::ErrorCode::UpstreamChallengeBlocked)
        {
            super::super::super::record_account_group_proxy_challenge(account);
        }
        let _ = super::super::super::affinity::mark_account_hard_quota_exhausted(
            context.storage(),
            account_id,
            hard_quota_headers.as_ref(),
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
                let request_body_bytes = match request_body.read_all_bytes() {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        log::warn!(
                            "event=gateway_affinity_finalize_skipped trace_id={} account_id={} reason=request_body_read_failed err={}",
                            trace_id,
                            account_id,
                            err
                        );
                        return Ok(());
                    }
                };
                if let Err(err) = super::super::super::affinity::finalize_affinity_success(
                    context.storage(),
                    resolution,
                    context.platform_key_hash(),
                    account_id,
                    request_body_bytes.as_ref(),
                    Some(completed_response_body),
                    response_adapter_label.as_str(),
                    context.protocol_type(),
                    Some(trace_id),
                ) {
                    log::warn!(
                        "event=gateway_affinity_finalize_failed trace_id={} account_id={} err={}",
                        trace_id,
                        account_id,
                        err
                    );
                    super::super::super::trace_log::log_affinity_finalize_error(
                        trace_id,
                        account_id,
                        Some(resolution.affinity_key.as_str()),
                        err.as_str(),
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
