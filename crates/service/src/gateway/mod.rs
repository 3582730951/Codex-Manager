use crate::storage_helpers::open_storage;

mod anchor_fingerprint;
#[path = "routing/conversation_binding.rs"]
mod conversation_binding;
#[path = "routing/cooldown.rs"]
mod cooldown;
mod error_response;
#[path = "routing/failover.rs"]
mod failover;
#[path = "observability/http_bridge/mod.rs"]
mod http_bridge;
#[path = "request/incoming_headers.rs"]
mod incoming_headers;
#[path = "request/local_count_tokens.rs"]
mod local_count_tokens;
#[path = "request/local_models.rs"]
mod local_models;
mod local_validation;
#[path = "observability/metrics.rs"]
mod metrics;
mod model_picker;
#[path = "auth/openai_fallback.rs"]
mod openai_fallback;
mod protocol_adapter;
#[path = "request/request_entry.rs"]
mod request_entry;
#[path = "routing/request_gate.rs"]
mod request_gate;
#[path = "request/request_helpers.rs"]
mod request_helpers;
#[path = "observability/request_log.rs"]
mod request_log;
#[path = "request/request_rewrite.rs"]
mod request_rewrite;
#[path = "routing/route_hint.rs"]
mod route_hint;
#[path = "routing/route_quality.rs"]
mod route_quality;
#[path = "core/runtime_config.rs"]
mod runtime_config;
#[path = "routing/scheduler.rs"]
mod scheduler;
#[path = "routing/selection.rs"]
mod selection;
#[path = "request/session_affinity.rs"]
mod session_affinity;
#[path = "auth/token_exchange.rs"]
mod token_exchange;
#[path = "observability/trace_log.rs"]
mod trace_log;
mod upstream;

use metrics::{
    account_inflight_count, acquire_account_inflight, begin_gateway_request,
    record_gateway_cooldown_mark, record_gateway_failover_attempt, record_gateway_request_outcome,
    AccountInFlightGuard,
};
pub(crate) use metrics::{
    begin_rpc_request, duration_to_millis, gateway_metrics_prometheus, record_usage_refresh_outcome,
};
use protocol_adapter::{
    adapt_request_for_protocol, adapt_upstream_response,
    adapt_upstream_response_with_tool_name_restore_map, build_anthropic_error_body,
    convert_openai_chat_stream_chunk_with_tool_name_restore_map,
    convert_openai_completions_stream_chunk, ResponseAdapter, ToolNameRestoreMap,
};
pub(super) use request_helpers::{
    is_html_content_type, is_upstream_challenge_response, normalize_models_path,
    parse_request_metadata,
};
#[cfg(test)]
use request_helpers::{should_drop_incoming_header, should_drop_incoming_header_for_failover};
use request_rewrite::{
    apply_request_overrides_with_forced_prompt_cache_key,
    apply_request_overrides_with_service_tier_and_forced_prompt_cache_key,
    apply_request_overrides_with_service_tier_and_prompt_cache_key, compute_upstream_url,
};
#[cfg(test)]
use upstream::config::normalize_upstream_base_url;
use upstream::config::{
    is_openai_api_base, resolve_upstream_base_url, resolve_upstream_fallback_base_url,
    should_try_openai_fallback, should_try_openai_fallback_by_status,
};
#[cfg(test)]
pub(super) use upstream::header_profile::{
    build_codex_compact_upstream_headers, build_codex_upstream_headers,
    CodexCompactUpstreamHeaderInput, CodexUpstreamHeaderInput,
};

// HTTP backend runtime metrics are exported via the gateway `/metrics` endpoint as well.
pub(crate) fn record_http_queue_capacity(normal_capacity: usize, stream_capacity: usize) {
    metrics::record_http_queue_capacity(normal_capacity, stream_capacity);
}

pub(crate) fn record_http_queue_enqueue(is_stream_queue: bool) {
    metrics::record_http_queue_enqueue(is_stream_queue);
}

pub(crate) fn record_http_queue_dequeue(is_stream_queue: bool) {
    metrics::record_http_queue_dequeue(is_stream_queue);
}

pub(crate) fn record_http_queue_enqueue_failure() {
    metrics::record_http_queue_enqueue_failure();
}
#[cfg(test)]
use cooldown::cooldown_reason_for_status;
use cooldown::{
    clear_account_cooldown, is_account_in_cooldown, mark_account_cooldown,
    mark_account_cooldown_for_status, CooldownReason,
};
#[cfg(test)]
pub(super) use failover::should_failover_after_refresh;
use failover::should_failover_from_cached_snapshot;
use http_bridge::respond_with_upstream;
pub(crate) use http_bridge::summarize_upstream_error_hint_from_body;
pub(crate) fn extract_identity_error_code_from_headers(
    headers: &reqwest::header::HeaderMap,
) -> Option<String> {
    headers
        .get("x-error-json")
        .and_then(|value| value.to_str().ok())
        .and_then(extract_identity_error_code_from_header_value)
}

fn extract_identity_error_code_from_header_value(raw: &str) -> Option<String> {
    if let Some(code) = extract_identity_error_code_from_error_json(raw) {
        return Some(code);
    }

    let decoded = decode_base64_header_value(raw.as_bytes())?;
    let decoded = String::from_utf8(decoded).ok()?;
    extract_identity_error_code_from_error_json(&decoded)
}

fn extract_identity_error_code_from_error_json(raw: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    value
        .get("identity_error_code")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            value
                .get("error")
                .and_then(serde_json::Value::as_object)
                .and_then(|error| error.get("code"))
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            value
                .get("error")
                .and_then(serde_json::Value::as_object)
                .and_then(|error| error.get("identity_error_code"))
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            value
                .get("details")
                .and_then(serde_json::Value::as_object)
                .and_then(|details| details.get("identity_error_code"))
                .and_then(serde_json::Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn decode_base64_header_value(input: &[u8]) -> Option<Vec<u8>> {
    fn decode_char(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }

    let filtered = input
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    if filtered.is_empty() || filtered.len() % 4 != 0 {
        return None;
    }

    let mut output = Vec::with_capacity(filtered.len() / 4 * 3);
    for chunk in filtered.chunks(4) {
        let a = decode_char(chunk[0])?;
        let b = decode_char(chunk[1])?;
        let c_pad = chunk[2] == b'=';
        let d_pad = chunk[3] == b'=';
        let c = if c_pad { 0 } else { decode_char(chunk[2])? };
        let d = if d_pad { 0 } else { decode_char(chunk[3])? };

        output.push((a << 2) | (b >> 4));
        if !c_pad {
            output.push((b << 4) | (c >> 2));
        }
        if !d_pad {
            output.push((c << 6) | d);
        }
    }

    Some(output)
}
pub(super) use incoming_headers::IncomingHeaderSnapshot;
use local_count_tokens::maybe_respond_local_count_tokens;
use local_models::maybe_respond_local_models;
pub(crate) use model_picker::fetch_models_for_picker;
use openai_fallback::try_openai_fallback;
pub(crate) use request_entry::handle_gateway_request;
use request_gate::{request_gate_lock, RequestGateAcquireError};
use request_log::write_request_log;
use route_hint::apply_route_strategy;
use route_quality::record_route_quality as record_route_quality_inner;
pub(crate) use runtime_config::fresh_upstream_client;
pub(crate) use runtime_config::front_proxy_max_body_bytes;
use runtime_config::{
    account_max_inflight_limit, fresh_upstream_client_for_account, trace_body_preview_max_bytes,
    upstream_client, upstream_client_for_account, upstream_stream_timeout, upstream_total_timeout,
    DEFAULT_GATEWAY_DEBUG,
};
use selection::collect_gateway_candidates;
#[cfg(test)]
use token_exchange::account_token_exchange_lock;
use token_exchange::resolve_openai_bearer_token;
use upstream::proxy::proxy_validated_request;

pub(crate) fn reload_runtime_config_from_env() {
    runtime_config::reload_from_env();
    selection::reload_from_env();
    request_gate::clear_runtime_state();
    cooldown::clear_runtime_state();
    route_quality::clear_runtime_state();
    scheduler::clear_runtime_state();
    route_hint::reload_from_env();
    upstream::config::reload_from_env();
    trace_log::reload_from_env();
    http_bridge::reload_from_env();
    protocol_adapter::prompt_cache::reload_runtime_state();
}

pub(crate) fn current_route_strategy() -> &'static str {
    route_hint::current_route_strategy()
}

pub(crate) fn set_route_strategy(strategy: &str) -> Result<&'static str, String> {
    let applied = route_hint::set_route_strategy(strategy)?;
    std::env::set_var("CODEXMANAGER_ROUTE_STRATEGY", applied);
    Ok(applied)
}

pub(crate) fn current_free_account_max_model() -> String {
    runtime_config::current_free_account_max_model()
}

pub(crate) fn request_compression_enabled() -> bool {
    runtime_config::request_compression_enabled()
}

pub(crate) fn current_originator() -> String {
    runtime_config::current_originator()
}

pub(crate) fn current_wire_originator() -> String {
    runtime_config::current_wire_originator()
}

pub(crate) fn current_codex_user_agent_version() -> String {
    runtime_config::current_codex_user_agent_version()
}

pub(crate) fn set_originator(originator: &str) -> Result<String, String> {
    runtime_config::set_originator(originator)
}

pub(crate) fn set_codex_user_agent_version(version: &str) -> Result<String, String> {
    runtime_config::set_codex_user_agent_version(version)
}

pub(crate) fn current_residency_requirement() -> Option<String> {
    runtime_config::current_residency_requirement()
}

pub(crate) fn set_residency_requirement(value: Option<&str>) -> Result<Option<String>, String> {
    runtime_config::set_residency_requirement(value)
}

pub(crate) fn current_codex_user_agent() -> String {
    runtime_config::current_codex_user_agent()
}

pub(crate) fn set_free_account_max_model(model: &str) -> Result<String, String> {
    runtime_config::set_free_account_max_model(model)
}

pub(crate) fn set_request_compression_enabled(enabled: bool) -> bool {
    runtime_config::set_request_compression_enabled(enabled)
}

pub(crate) fn strict_request_param_allowlist_enabled() -> bool {
    runtime_config::strict_request_param_allowlist_enabled()
}

pub(crate) fn current_upstream_proxy_url() -> Option<String> {
    runtime_config::upstream_proxy_url()
}

pub(crate) fn set_upstream_proxy_url(proxy_url: Option<&str>) -> Result<Option<String>, String> {
    let applied = runtime_config::set_upstream_proxy_url(proxy_url)?;
    // 中文注释：用量轮询和 token 刷新复用独立 HTTP client，代理变更后同步重建，避免继续走旧网络路径。
    crate::usage_http::reload_usage_http_client_from_env();
    Ok(applied)
}

pub(crate) fn current_upstream_stream_timeout_ms() -> u64 {
    runtime_config::current_upstream_stream_timeout_ms()
}

pub(crate) fn set_upstream_stream_timeout_ms(timeout_ms: u64) -> u64 {
    runtime_config::set_upstream_stream_timeout_ms(timeout_ms)
}

pub(crate) fn current_sse_keepalive_interval_ms() -> u64 {
    http_bridge::current_sse_keepalive_interval_ms()
}

pub(crate) fn set_sse_keepalive_interval_ms(interval_ms: u64) -> Result<u64, String> {
    http_bridge::set_sse_keepalive_interval_ms(interval_ms)
}

#[cfg(test)]
pub(crate) fn gateway_runtime_test_guard() -> std::sync::MutexGuard<'static, ()> {
    runtime_config::gateway_runtime_test_guard()
}

pub(crate) fn manual_preferred_account() -> Option<String> {
    route_hint::get_manual_preferred_account()
}

pub(crate) fn set_manual_preferred_account(account_id: &str) -> Result<(), String> {
    let id = account_id.trim();
    if id.is_empty() {
        return Err("accountId is required".to_string());
    }
    let storage = open_storage().ok_or_else(|| "storage not initialized".to_string())?;
    let candidates = collect_gateway_candidates(&storage)?;
    let found = candidates.iter().any(|(account, _)| account.id == id);
    if !found {
        return Err("account is not available for routing".to_string());
    }
    route_hint::set_manual_preferred_account(id)
}

pub(crate) fn clear_manual_preferred_account() {
    route_hint::clear_manual_preferred_account();
}

pub(crate) fn clear_manual_preferred_account_if(account_id: &str) -> bool {
    route_hint::clear_manual_preferred_account_if(account_id)
}

pub(crate) fn record_route_quality(account_id: &str, status_code: u16) {
    let health = record_route_quality_inner(account_id, status_code);
    scheduler::record_route_health(account_id, health);
}

pub(crate) fn record_scheduler_assignment(account_id: &str) {
    scheduler::record_assignment(account_id);
}

pub(crate) fn record_scheduler_feedback(account_id: &str, feedback: scheduler::SchedulerFeedback) {
    scheduler::record_feedback(account_id, feedback);
}

pub(crate) fn rebalance_scheduler_candidates(
    storage: &codexmanager_core::storage::Storage,
    candidates: &mut Vec<(
        codexmanager_core::storage::Account,
        codexmanager_core::storage::Token,
    )>,
    static_limit: usize,
    preserve_head: bool,
) -> std::collections::HashMap<String, usize> {
    scheduler::rebalance_candidates(storage, candidates, static_limit, preserve_head)
}

pub(crate) fn wait_for_scheduler_candidate_window(
    candidates: &[(
        codexmanager_core::storage::Account,
        codexmanager_core::storage::Token,
    )],
    account_dynamic_limits: &std::collections::HashMap<String, usize>,
    deadline: Option<std::time::Instant>,
) -> bool {
    scheduler::wait_for_candidate_window(candidates, account_dynamic_limits, deadline)
}

pub(crate) fn scheduler_store_usage_snapshot(
    record: &codexmanager_core::storage::UsageSnapshotRecord,
) {
    scheduler::store_usage_snapshot(record);
}

pub(crate) fn scheduler_set_account_inflight(
    account_id: &str,
    inflight: usize,
    wake_waiters: bool,
) {
    scheduler::set_account_inflight(account_id, inflight, wake_waiters);
}

pub(crate) fn scheduler_set_account_cooldown_until(
    account_id: &str,
    cooldown_until: Option<i64>,
    wake_waiters: bool,
) {
    scheduler::set_account_cooldown_until(account_id, cooldown_until, wake_waiters);
}

pub(crate) fn scheduler_account_inflight_count(account_id: &str) -> usize {
    scheduler::cached_account_inflight(account_id)
}

pub(crate) fn scheduler_account_in_cooldown(account_id: &str) -> bool {
    scheduler::cached_account_in_cooldown(account_id)
}

#[cfg(test)]
#[path = "../../tests/gateway/availability/mod.rs"]
mod availability_tests;
