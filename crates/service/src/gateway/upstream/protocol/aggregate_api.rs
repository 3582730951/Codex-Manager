use bytes::Bytes;
use codexmanager_core::storage::{AggregateApi, Storage};
use reqwest::header::{HeaderName, HeaderValue};
use std::time::Instant;
use tiny_http::Request;

use crate::aggregate_api::{AGGREGATE_API_PROVIDER_CLAUDE, AGGREGATE_API_PROVIDER_CODEX};
use crate::gateway::http_bridge::UpstreamCompletionState;
use crate::gateway::request_log::RequestLogUsage;

const AGGREGATE_API_RETRY_ATTEMPTS_PER_CHANNEL: usize = 3;

fn should_skip_forward_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization"
            | "x-api-key"
            | "api-key"
            | "x-codex-cli-affinity-id"
            | "x-codex-internal-client-entity"
            | "x-codex-internal-entity-ts"
            | "x-codex-internal-entity-nonce"
            | "x-codex-internal-entity-sig"
            | "x-codex-internal-peer-ip"
            | "x-codex-internal-hop-sig"
            | "content-length"
            | "connection"
            | "proxy-authorization"
            | "proxy-authenticate"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
    )
}

fn respond_error(request: Request, status: u16, message: &str, trace_id: Option<&str>) {
    let response = super::super::super::error_response::terminal_text_response(
        status,
        message.to_string(),
        trace_id,
    );
    let _ = request.respond(response);
}

fn normalize_candidate_order(mut candidates: Vec<AggregateApi>) -> Vec<AggregateApi> {
    candidates.sort_by(|left, right| {
        left.sort
            .cmp(&right.sort)
            .then(right.created_at.cmp(&left.created_at))
            .then(left.id.cmp(&right.id))
    });
    candidates
}

fn normalize_provider_type_value(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "claude" | "anthropic" | "anthropic_native" | "claude_code" => {
            AGGREGATE_API_PROVIDER_CLAUDE.to_string()
        }
        _ => AGGREGATE_API_PROVIDER_CODEX.to_string(),
    }
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

fn aggregate_api_failure_message(
    status_code: u16,
    body: &[u8],
    request_id: Option<&str>,
    cf_ray: Option<&str>,
    auth_error: Option<&str>,
    identity_error_code: Option<&str>,
) -> String {
    let mut parts =
        vec![
            crate::gateway::summarize_upstream_error_hint_from_body(status_code, body)
                .unwrap_or_else(|| format!("aggregate api upstream status={status_code}")),
        ];
    if let Some(request_id) = request_id.map(str::trim).filter(|value| !value.is_empty()) {
        parts.push(format!("request_id={request_id}"));
    }
    if let Some(cf_ray) = cf_ray.map(str::trim).filter(|value| !value.is_empty()) {
        parts.push(format!("cf_ray={cf_ray}"));
    }
    if let Some(auth_error) = auth_error.map(str::trim).filter(|value| !value.is_empty()) {
        parts.push(format!("auth_error={auth_error}"));
    }
    if let Some(identity_error_code) = identity_error_code
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("identity_error_code={identity_error_code}"));
    }
    if parts.len() == 1 {
        parts.remove(0)
    } else {
        format!("{} [{}]", parts.remove(0), parts.join(", "))
    }
}

fn build_aggregate_api_request(
    client: &reqwest::blocking::Client,
    request: &Request,
    method: &reqwest::Method,
    url: reqwest::Url,
    body: &Bytes,
    secret: &str,
    request_deadline: Option<Instant>,
    is_stream: bool,
) -> Result<reqwest::blocking::RequestBuilder, String> {
    let mut builder = client.request(method.clone(), url);
    if let Some(timeout) =
        super::super::support::deadline::send_timeout(request_deadline, is_stream)
    {
        builder = builder.timeout(timeout);
    }
    let request_headers = request.headers().to_vec();
    for header in &request_headers {
        if should_skip_forward_header(header.field.as_str().into()) {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(header.field.as_str().as_bytes()),
            HeaderValue::from_str(header.value.as_str()),
        ) {
            builder = builder.header(name, value);
        }
    }
    builder = builder.header(
        HeaderName::from_static("authorization"),
        HeaderValue::from_str(format!("Bearer {}", secret).as_str())
            .map_err(|_| "invalid aggregate api secret".to_string())?,
    );
    if !body.is_empty() {
        builder = builder.body(body.clone());
    }
    Ok(builder)
}

pub(crate) fn resolve_aggregate_api_rotation_candidates(
    storage: &Storage,
    protocol_type: &str,
    aggregate_api_id: Option<&str>,
) -> Result<Vec<AggregateApi>, String> {
    let provider_type = if protocol_type == "anthropic_native" {
        AGGREGATE_API_PROVIDER_CLAUDE
    } else {
        AGGREGATE_API_PROVIDER_CODEX
    };

    let mut candidates = storage
        .list_aggregate_apis()
        .map_err(|err| err.to_string())?
        .into_iter()
        .filter(|api| {
            api.status == "active"
                && normalize_provider_type_value(api.provider_type.as_str()) == provider_type
        })
        .collect::<Vec<_>>();
    candidates = normalize_candidate_order(candidates);

    if let Some(api_id) = aggregate_api_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(preferred) = storage
            .find_aggregate_api_by_id(api_id)
            .map_err(|err| err.to_string())?
        {
            candidates.retain(|api| api.id != preferred.id);
            candidates.insert(0, preferred);
        }
    }

    if candidates.is_empty() {
        Err(format!(
            "aggregate api not found for provider {provider_type}"
        ))
    } else {
        Ok(candidates)
    }
}

#[allow(clippy::too_many_arguments)]
pub(in super::super) fn proxy_aggregate_request(
    request: Request,
    storage: &Storage,
    trace_id: &str,
    key_id: &str,
    original_path: &str,
    path: &str,
    request_method: &str,
    method: &reqwest::Method,
    body: &Bytes,
    is_stream: bool,
    response_adapter: super::super::super::ResponseAdapter,
    model_for_log: Option<&str>,
    reasoning_for_log: Option<&str>,
    aggregate_api_candidates: Vec<AggregateApi>,
    request_deadline: Option<Instant>,
    started_at: Instant,
) -> Result<(), String> {
    if aggregate_api_candidates.is_empty() {
        let message = "aggregate api not found".to_string();
        super::super::super::record_gateway_request_outcome(path, 404, Some("aggregate_api"));
        super::super::super::trace_log::log_request_final(
            trace_id,
            404,
            Some(key_id),
            None,
            Some(message.as_str()),
            started_at.elapsed().as_millis(),
        );
        let request = request;
        respond_error(request, 404, message.as_str(), Some(trace_id));
        return Ok(());
    }

    let client = super::super::super::fresh_upstream_client();
    let mut request = Some(request);
    let mut attempted_aggregate_api_ids = Vec::new();
    let mut last_attempt_url: Option<String> = None;
    let mut last_attempt_supplier_name: Option<String> = None;
    let mut last_attempt_error: Option<String> = None;
    let mut last_failure_status = 502u16;

    let total_candidates = aggregate_api_candidates.len();
    for (candidate_idx, candidate) in aggregate_api_candidates.into_iter().enumerate() {
        attempted_aggregate_api_ids.push(candidate.id.clone());
        let candidate_supplier_name = candidate.supplier_name.clone();
        let candidate_url = candidate.url.clone();
        let Some(secret) = storage
            .find_aggregate_api_secret_by_id(candidate.id.as_str())
            .map_err(|err| err.to_string())?
        else {
            last_attempt_url = Some(candidate_url.clone());
            last_attempt_supplier_name = candidate_supplier_name.clone();
            last_attempt_error = Some("aggregate api secret not found".to_string());
            last_failure_status = 403;
            continue;
        };

        let mut succeeded = false;
        for attempt_idx in 0..=AGGREGATE_API_RETRY_ATTEMPTS_PER_CHANNEL {
            if super::super::support::deadline::is_expired(request_deadline) {
                let message = "aggregate api request timeout".to_string();
                let request = request
                    .take()
                    .expect("request should still be available for timeout response");
                super::super::super::record_gateway_request_outcome(
                    path,
                    504,
                    Some("aggregate_api"),
                );
                super::super::super::trace_log::log_request_final(
                    trace_id,
                    504,
                    Some(key_id),
                    Some(candidate_url.as_str()),
                    Some(message.as_str()),
                    started_at.elapsed().as_millis(),
                );
                super::super::super::write_request_log(
                    storage,
                    super::super::super::request_log::RequestLogTraceContext {
                        trace_id: Some(trace_id),
                        original_path: Some(original_path),
                        adapted_path: Some(path),
                        response_adapter: Some(response_adapter),
                        aggregate_api_supplier_name: candidate_supplier_name.as_deref(),
                        aggregate_api_url: Some(candidate_url.as_str()),
                        attempted_aggregate_api_ids: Some(attempted_aggregate_api_ids.as_slice()),
                        ..Default::default()
                    },
                    Some(key_id),
                    None,
                    path,
                    request_method,
                    model_for_log,
                    reasoning_for_log,
                    Some(candidate_url.as_str()),
                    Some(504),
                    RequestLogUsage::default(),
                    Some(message.as_str()),
                    Some(started_at.elapsed().as_millis()),
                );
                respond_error(request, 504, message.as_str(), Some(trace_id));
                return Ok(());
            }

            let url =
                match reqwest::Url::parse(candidate_url.as_str()).and_then(|url| url.join(path)) {
                    Ok(url) => url,
                    Err(_) => {
                        last_attempt_url = Some(candidate_url.clone());
                        last_attempt_supplier_name = candidate_supplier_name.clone();
                        last_attempt_error = Some("invalid aggregate api url".to_string());
                        last_failure_status = 502;
                        break;
                    }
                };

            let builder = build_aggregate_api_request(
                &client,
                request.as_ref().expect("request should still be available"),
                method,
                url.clone(),
                body,
                secret.as_str(),
                request_deadline,
                is_stream,
            )?;

            let attempt_started_at = Instant::now();
            let upstream = match builder.send() {
                Ok(resp) => {
                    let duration_ms =
                        super::super::super::duration_to_millis(attempt_started_at.elapsed());
                    super::super::super::metrics::record_gateway_upstream_attempt(
                        duration_ms,
                        false,
                    );
                    resp
                }
                Err(err) => {
                    let duration_ms =
                        super::super::super::duration_to_millis(attempt_started_at.elapsed());
                    super::super::super::metrics::record_gateway_upstream_attempt(
                        duration_ms,
                        true,
                    );
                    let message = format!("aggregate api upstream error: {err}");
                    last_attempt_url = Some(url.as_str().to_string());
                    last_attempt_supplier_name = candidate_supplier_name.clone();
                    last_attempt_error = Some(message);
                    last_failure_status = 502;
                    if attempt_idx < AGGREGATE_API_RETRY_ATTEMPTS_PER_CHANNEL {
                        continue;
                    }
                    break;
                }
            };

            if !upstream.status().is_success() {
                let status_code = upstream.status().as_u16();
                let upstream_request_id = first_upstream_header(
                    upstream.headers(),
                    &["x-request-id", "x-oai-request-id"],
                );
                let upstream_cf_ray = first_upstream_header(upstream.headers(), &["cf-ray"]);
                let upstream_auth_error =
                    first_upstream_header(upstream.headers(), &["x-openai-authorization-error"]);
                let upstream_identity_error_code =
                    crate::gateway::extract_identity_error_code_from_headers(upstream.headers());
                let upstream_body = upstream
                    .bytes()
                    .map_err(|err| format!("read upstream body failed: {err}"))?;
                let message = aggregate_api_failure_message(
                    status_code,
                    upstream_body.as_ref(),
                    upstream_request_id.as_deref(),
                    upstream_cf_ray.as_deref(),
                    upstream_auth_error.as_deref(),
                    upstream_identity_error_code.as_deref(),
                );
                last_attempt_url = Some(url.as_str().to_string());
                last_attempt_supplier_name = candidate_supplier_name.clone();
                last_attempt_error = Some(message);
                last_failure_status = 502;
                if attempt_idx < AGGREGATE_API_RETRY_ATTEMPTS_PER_CHANNEL {
                    continue;
                }
                break;
            }

            let inflight_guard = super::super::super::acquire_account_inflight(key_id);
            let bridge = super::super::super::respond_with_upstream(
                request
                    .take()
                    .expect("request should be available before bridge"),
                upstream,
                inflight_guard,
                response_adapter,
                path,
                None,
                is_stream,
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
                is_stream,
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
            let bridge_ok = bridge.is_ok(is_stream);
            let mut final_error = bridge.upstream_error_hint.clone();
            if final_error.is_none() && !bridge_ok {
                final_error =
                    Some(bridge.error_message(is_stream).unwrap_or_else(|| {
                        "aggregate api upstream response incomplete".to_string()
                    }));
            }
            let status_code = if bridge.is_client_disconnect() {
                499
            } else if let Some(delivered_status_code) = bridge.delivered_status_code {
                delivered_status_code
            } else if bridge_ok {
                200
            } else if is_stream
                && matches!(
                    bridge.upstream_completion_state,
                    UpstreamCompletionState::EofWithoutTerminal
                        | UpstreamCompletionState::ReaderError
                )
            {
                502
            } else if final_error.is_some() {
                502
            } else {
                502
            };
            let usage = bridge.usage;

            super::super::super::record_gateway_request_outcome(
                path,
                status_code,
                Some("aggregate_api"),
            );
            super::super::super::trace_log::log_request_final(
                trace_id,
                status_code,
                Some(key_id),
                Some(url.as_str()),
                final_error.as_deref(),
                started_at.elapsed().as_millis(),
            );
            super::super::super::write_request_log(
                storage,
                super::super::super::request_log::RequestLogTraceContext {
                    trace_id: Some(trace_id),
                    original_path: Some(original_path),
                    adapted_path: Some(path),
                    response_adapter: Some(response_adapter),
                    aggregate_api_supplier_name: candidate_supplier_name.as_deref(),
                    aggregate_api_url: Some(candidate_url.as_str()),
                    attempted_aggregate_api_ids: Some(attempted_aggregate_api_ids.as_slice()),
                    ..Default::default()
                },
                Some(key_id),
                None,
                path,
                request_method,
                model_for_log,
                reasoning_for_log,
                Some(url.as_str()),
                Some(status_code),
                RequestLogUsage {
                    input_tokens: usage.input_tokens,
                    cached_input_tokens: usage.cached_input_tokens,
                    output_tokens: usage.output_tokens,
                    total_tokens: usage.total_tokens,
                    reasoning_output_tokens: usage.reasoning_output_tokens,
                },
                final_error.as_deref(),
                Some(started_at.elapsed().as_millis()),
            );
            succeeded = true;
            break;
        }

        if succeeded {
            return Ok(());
        }

        if candidate_idx + 1 < total_candidates {
            super::super::super::record_gateway_failover_attempt();
        }
    }

    let message =
        last_attempt_error.unwrap_or_else(|| "aggregate api upstream response failed".to_string());
    let status_code = last_failure_status;
    let request = request
        .take()
        .expect("request should still be available for failure response");
    super::super::super::record_gateway_request_outcome(path, status_code, Some("aggregate_api"));
    super::super::super::trace_log::log_request_final(
        trace_id,
        status_code,
        Some(key_id),
        last_attempt_url.as_deref(),
        Some(message.as_str()),
        started_at.elapsed().as_millis(),
    );
    super::super::super::write_request_log(
        storage,
        super::super::super::request_log::RequestLogTraceContext {
            trace_id: Some(trace_id),
            original_path: Some(original_path),
            adapted_path: Some(path),
            response_adapter: Some(response_adapter),
            aggregate_api_supplier_name: last_attempt_supplier_name.as_deref(),
            aggregate_api_url: last_attempt_url.as_deref(),
            attempted_aggregate_api_ids: Some(attempted_aggregate_api_ids.as_slice()),
            ..Default::default()
        },
        Some(key_id),
        None,
        path,
        request_method,
        model_for_log,
        reasoning_for_log,
        last_attempt_url.as_deref(),
        Some(status_code),
        RequestLogUsage::default(),
        Some(message.as_str()),
        Some(started_at.elapsed().as_millis()),
    );
    respond_error(request, status_code, message.as_str(), Some(trace_id));
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn skip_forward_header_drops_local_cli_affinity_header() {
        assert!(super::should_skip_forward_header("x-codex-cli-affinity-id"));
    }

    #[test]
    fn skip_forward_header_keeps_regular_content_headers() {
        assert!(!super::should_skip_forward_header("content-type"));
    }
}
