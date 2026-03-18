use codexmanager_core::storage::{Account, Token};
use std::sync::atomic::{AtomicU64, Ordering};

use super::super::super::IncomingHeaderSnapshot;
use crate::apikey_profile::PROTOCOL_ANTHROPIC_NATIVE;

pub(in super::super) struct UpstreamRequestSetup {
    pub(in super::super) upstream_base: String,
    pub(in super::super) upstream_fallback_base: Option<String>,
    pub(in super::super) url: String,
    pub(in super::super) url_alt: Option<String>,
    pub(in super::super) upstream_cookie: Option<String>,
    pub(in super::super) flow_key: String,
    pub(in super::super) candidate_count: usize,
    pub(in super::super) account_max_inflight: usize,
    pub(in super::super) anthropic_has_prompt_cache_key: bool,
    pub(in super::super) has_sticky_fallback_session: bool,
    pub(in super::super) has_sticky_fallback_conversation: bool,
    pub(in super::super) has_body_encrypted_content: bool,
}

pub(in super::super) fn prepare_request_setup(
    path: &str,
    protocol_type: &str,
    has_prompt_cache_key: bool,
    prompt_cache_key: Option<&str>,
    incoming_headers: &IncomingHeaderSnapshot,
    body: &bytes::Bytes,
    candidates: &mut Vec<(Account, Token)>,
    key_id: &str,
    model_for_log: Option<&str>,
    trace_id: &str,
) -> UpstreamRequestSetup {
    let upstream_base = super::super::super::resolve_upstream_base_url();
    let upstream_fallback_base =
        super::super::super::resolve_upstream_fallback_base_url(upstream_base.as_str());
    let (url, url_alt) =
        super::super::super::request_rewrite::compute_upstream_url(upstream_base.as_str(), path);
    let upstream_cookie = super::super::super::upstream_cookie();
    let account_max_inflight = super::super::super::account_max_inflight_limit();
    let anthropic_has_prompt_cache_key =
        protocol_type == PROTOCOL_ANTHROPIC_NATIVE && has_prompt_cache_key;
    let flow_key = resolve_flow_key(prompt_cache_key, incoming_headers);
    super::super::super::apply_route_strategy(
        candidates,
        super::super::super::RouteSelectionContext::new(key_id, model_for_log, flow_key.as_str()),
    );
    let candidate_count = candidates.len();
    let candidate_order = candidates
        .iter()
        .map(|(account, _)| format!("{}#sort={}", account.id, account.sort))
        .collect::<Vec<_>>();
    super::super::super::trace_log::log_candidate_pool(
        trace_id,
        key_id,
        super::super::super::current_route_strategy(),
        flow_key.as_str(),
        candidate_order.as_slice(),
    );

    UpstreamRequestSetup {
        upstream_base,
        upstream_fallback_base,
        url,
        url_alt,
        upstream_cookie,
        flow_key,
        candidate_count,
        account_max_inflight,
        anthropic_has_prompt_cache_key,
        has_sticky_fallback_session:
            super::super::header_profile::derive_sticky_session_id_from_headers(incoming_headers)
                .is_some(),
        has_sticky_fallback_conversation:
            super::super::header_profile::derive_sticky_conversation_id_from_headers(
                incoming_headers,
            )
            .is_some(),
        has_body_encrypted_content:
            super::super::support::payload_rewrite::body_has_encrypted_content_hint(body.as_ref()),
    }
}

fn resolve_flow_key(
    prompt_cache_key: Option<&str>,
    incoming_headers: &IncomingHeaderSnapshot,
) -> String {
    prompt_cache_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| incoming_headers.session_id().map(str::to_string))
        .or_else(|| incoming_headers.conversation_id().map(str::to_string))
        .or_else(|| incoming_headers.client_request_id().map(str::to_string))
        .unwrap_or_else(next_local_route_sequence)
}

fn next_local_route_sequence() -> String {
    static ROUTE_REQUEST_SEQ: AtomicU64 = AtomicU64::new(1);
    format!(
        "local_req_{}",
        ROUTE_REQUEST_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}
