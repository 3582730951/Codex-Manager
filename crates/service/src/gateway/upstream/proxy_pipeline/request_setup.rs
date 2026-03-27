use std::collections::HashMap;

use codexmanager_core::storage::{Account, ConversationBinding, Token};

use super::super::super::IncomingHeaderSnapshot;
use crate::apikey_profile::PROTOCOL_ANTHROPIC_NATIVE;
use crate::gateway::conversation_binding::ConversationRoutingContext;

pub(in super::super) struct UpstreamRequestSetup {
    pub(in super::super) upstream_base: String,
    pub(in super::super) upstream_fallback_base: Option<String>,
    pub(in super::super) url: String,
    pub(in super::super) url_alt: Option<String>,
    pub(in super::super) candidate_count: usize,
    pub(in super::super) account_max_inflight: usize,
    pub(in super::super) account_dynamic_limits: HashMap<String, usize>,
    pub(in super::super) anthropic_has_prompt_cache_key: bool,
    pub(in super::super) has_sticky_fallback_session: bool,
    pub(in super::super) has_sticky_fallback_conversation: bool,
    pub(in super::super) has_body_encrypted_content: bool,
    pub(in super::super) conversation_routing: Option<ConversationRoutingContext>,
}

pub(in super::super) fn prepare_request_setup(
    storage: &codexmanager_core::storage::Storage,
    path: &str,
    protocol_type: &str,
    has_prompt_cache_key: bool,
    incoming_headers: &IncomingHeaderSnapshot,
    body: &bytes::Bytes,
    candidates: &mut Vec<(Account, Token)>,
    key_id: &str,
    platform_key_hash: &str,
    local_conversation_id: Option<&str>,
    conversation_binding: Option<&ConversationBinding>,
    model_for_log: Option<&str>,
    trace_id: &str,
) -> UpstreamRequestSetup {
    let upstream_base = super::super::super::resolve_upstream_base_url();
    let upstream_fallback_base =
        super::super::super::resolve_upstream_fallback_base_url(upstream_base.as_str());
    let (url, url_alt) =
        super::super::super::request_rewrite::compute_upstream_url(upstream_base.as_str(), path);
    let candidate_count = candidates.len();
    let account_max_inflight = super::super::super::account_max_inflight_limit();
    let anthropic_has_prompt_cache_key =
        protocol_type == PROTOCOL_ANTHROPIC_NATIVE && has_prompt_cache_key;
    let conversation_routing =
        super::super::super::conversation_binding::prepare_conversation_routing(
            platform_key_hash,
            local_conversation_id,
            conversation_binding,
            candidates,
        );
    let rotation_plan = super::super::super::conversation_binding::apply_candidate_rotation(
        candidates,
        conversation_routing.as_ref(),
        key_id,
        model_for_log,
    );
    let preserve_head = conversation_routing.as_ref().is_some_and(|routing| {
        routing.binding_selected || routing.manual_preferred_account_id.is_some()
    });
    let account_dynamic_limits = super::super::super::rebalance_scheduler_candidates(
        storage,
        candidates,
        account_max_inflight,
        preserve_head,
    );
    let candidate_order = candidates
        .iter()
        .map(|(account, _)| format!("{}#sort={}", account.id, account.sort))
        .collect::<Vec<_>>();
    super::super::super::trace_log::log_candidate_pool(
        trace_id,
        key_id,
        rotation_plan.strategy_label,
        rotation_plan.source.as_str(),
        rotation_plan.strategy_applied,
        candidate_order.as_slice(),
    );

    UpstreamRequestSetup {
        upstream_base,
        upstream_fallback_base,
        url,
        url_alt,
        candidate_count,
        account_max_inflight,
        account_dynamic_limits,
        anthropic_has_prompt_cache_key,
        has_sticky_fallback_session: false,
        has_sticky_fallback_conversation:
            super::super::header_profile::derive_sticky_conversation_id_from_headers(
                incoming_headers,
            )
            .is_some(),
        has_body_encrypted_content:
            super::super::support::payload_rewrite::body_has_encrypted_content_hint(body.as_ref()),
        conversation_routing,
    }
}
