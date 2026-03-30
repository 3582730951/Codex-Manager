use std::collections::HashMap;

use bytes::Bytes;
use codexmanager_core::storage::{Account, Storage, Token};

use super::super::super::IncomingHeaderSnapshot;
use crate::apikey_profile::PROTOCOL_ANTHROPIC_NATIVE;
use crate::gateway::affinity::{
    AffinityRoutingResolution, ClientEntityMode, ClientEntityRequestPreflight,
};
use crate::gateway::conversation_binding::ConversationRoutingContext;
use crate::gateway::upstream::config::normalize_upstream_base_url;

#[derive(Debug, Clone)]
pub(in super::super) enum RequestRoutingState {
    PersistentAffinity(AffinityRoutingResolution),
    LegacyConversation(ConversationRoutingContext),
    PeerRuntimeOnly,
    StatelessNoLegacy,
}

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
    pub(in super::super) request_body_override: Option<Bytes>,
    pub(in super::super) routing_state: RequestRoutingState,
    pub(in super::super) peer_runtime_key: Option<String>,
}

impl UpstreamRequestSetup {
    pub(in super::super) fn persistent_affinity_resolution(
        &self,
    ) -> Option<&AffinityRoutingResolution> {
        match &self.routing_state {
            RequestRoutingState::PersistentAffinity(resolution) => Some(resolution),
            _ => None,
        }
    }

    pub(in super::super) fn legacy_conversation_routing(
        &self,
    ) -> Option<&ConversationRoutingContext> {
        match &self.routing_state {
            RequestRoutingState::LegacyConversation(routing) => Some(routing),
            _ => None,
        }
    }

    pub(in super::super) fn records_persistent_affinity(&self) -> bool {
        matches!(self.routing_state, RequestRoutingState::PersistentAffinity(_))
    }

    pub(in super::super) fn records_legacy_conversation_binding(&self) -> bool {
        matches!(self.routing_state, RequestRoutingState::LegacyConversation(_))
    }
}

#[allow(clippy::too_many_arguments)]
pub(in super::super) fn prepare_request_setup(
    storage: &Storage,
    original_path: &str,
    path: &str,
    response_adapter: crate::gateway::ResponseAdapter,
    protocol_type: &str,
    explicit_upstream_base_url: Option<&str>,
    request_preflight: &ClientEntityRequestPreflight,
    has_prompt_cache_key: bool,
    incoming_headers: &IncomingHeaderSnapshot,
    body: &bytes::Bytes,
    candidates: &mut Vec<(Account, Token)>,
    key_id: &str,
    platform_key_hash: &str,
    local_conversation_id: Option<&str>,
    model_for_log: Option<&str>,
    trace_id: &str,
) -> Result<UpstreamRequestSetup, String> {
    let upstream_base = explicit_upstream_base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_upstream_base_url)
        .unwrap_or_else(super::super::super::resolve_upstream_base_url);
    let upstream_fallback_base =
        super::super::super::resolve_upstream_fallback_base_url(upstream_base.as_str());
    let (url, url_alt) =
        super::super::super::request_rewrite::compute_upstream_url(upstream_base.as_str(), path);
    let account_max_inflight = super::super::super::account_max_inflight_limit();
    let anthropic_has_prompt_cache_key =
        protocol_type == PROTOCOL_ANTHROPIC_NATIVE && has_prompt_cache_key;
    let has_sticky_fallback_conversation =
        super::super::header_profile::derive_sticky_conversation_id_from_headers(incoming_headers)
            .is_some();
    let has_body_encrypted_content =
        super::super::support::payload_rewrite::body_has_encrypted_content_hint(body.as_ref());
    let peer_runtime_key = request_preflight.trusted_peer_runtime_key.clone();

    let persistent_affinity = match request_preflight.mode {
        ClientEntityMode::EdgeEnforced => {
            if request_preflight.trusted_durable_affinity.is_none() {
                None
            } else {
                super::super::super::affinity::resolve_enforced_routing(
                    storage,
                    incoming_headers,
                    original_path,
                    path,
                    body,
                    candidates,
                    key_id,
                    platform_key_hash,
                    local_conversation_id,
                    model_for_log,
                    response_adapter,
                    request_preflight.trusted_durable_affinity.clone(),
                    false,
                    false,
                )?
            }
        }
        ClientEntityMode::Off | ClientEntityMode::DockerPeerRuntime => {
            super::super::super::affinity::resolve_enforced_routing(
                storage,
                incoming_headers,
                original_path,
                path,
                body,
                candidates,
                key_id,
                platform_key_hash,
                local_conversation_id,
                model_for_log,
                response_adapter,
                None,
                true,
                true,
            )?
        }
    };
    if let Some(resolution) = persistent_affinity {
        let strategy_label = if resolution.requires_replay {
            "affinity_replay"
        } else {
            "affinity_enforce"
        };
        candidates.retain(|(account, _)| {
            resolution
                .candidate_account_ids
                .iter()
                .any(|candidate_id| candidate_id == &account.id)
        });
        let candidate_count = candidates.len();
        let candidate_order = candidates
            .iter()
            .map(|(account, _)| format!("{}#sort={}", account.id, account.sort))
            .collect::<Vec<_>>();
        super::super::super::trace_log::log_candidate_pool(
            trace_id,
            key_id,
            strategy_label,
            resolution.affinity_source,
            true,
            candidate_order.as_slice(),
        );
        return Ok(UpstreamRequestSetup {
            upstream_base,
            upstream_fallback_base,
            url,
            url_alt,
            candidate_count,
            account_max_inflight,
            account_dynamic_limits: HashMap::new(),
            anthropic_has_prompt_cache_key,
            has_sticky_fallback_session: false,
            has_sticky_fallback_conversation,
            has_body_encrypted_content,
            request_body_override: resolution.request_body_override.clone(),
            routing_state: RequestRoutingState::PersistentAffinity(resolution),
            peer_runtime_key,
        });
    }

    if request_preflight.legacy_allowed {
        let conversation_binding = super::super::super::conversation_binding::load_conversation_binding(
            storage,
            platform_key_hash,
            local_conversation_id,
        )?;
        let effective_thread_anchor = super::super::super::conversation_binding::effective_thread_anchor(
            local_conversation_id,
            conversation_binding.as_ref(),
        );
        let request_body_override = effective_thread_anchor.as_ref().map(|thread_anchor| {
            Bytes::from(
                super::super::super::apply_request_overrides_with_service_tier_and_forced_prompt_cache_key(
                    path,
                    body.to_vec(),
                    None,
                    None,
                    None,
                    Some(upstream_base.as_str()),
                    Some(thread_anchor.as_str()),
                ),
            )
        });
        let conversation_routing =
            super::super::super::conversation_binding::prepare_conversation_routing(
                platform_key_hash,
                local_conversation_id,
                conversation_binding.as_ref(),
                candidates,
            );
        if let Some(routing) = conversation_routing {
            let rotation_plan = super::super::super::conversation_binding::apply_candidate_rotation(
                candidates,
                Some(&routing),
                key_id,
                model_for_log,
            );
            let preserve_head = routing.binding_selected || routing.manual_preferred_account_id.is_some();
            let account_dynamic_limits = super::super::super::rebalance_scheduler_candidates(
                storage,
                candidates,
                account_max_inflight,
                preserve_head,
            );
            let candidate_count = candidates.len();
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
            return Ok(UpstreamRequestSetup {
                upstream_base,
                upstream_fallback_base,
                url,
                url_alt,
                candidate_count,
                account_max_inflight,
                account_dynamic_limits,
                anthropic_has_prompt_cache_key,
                has_sticky_fallback_session: false,
                has_sticky_fallback_conversation,
                has_body_encrypted_content,
                request_body_override,
                routing_state: RequestRoutingState::LegacyConversation(routing),
                peer_runtime_key,
            });
        }
    }

    let peer_runtime_hint =
        super::super::super::affinity::resolve_peer_runtime_hint(peer_runtime_key.as_deref());
    if let Some(hint) = peer_runtime_hint.as_ref() {
        if let Some(account_id) = hint.pinned_account_id.as_deref() {
            rotate_candidates_to_account(candidates, account_id);
        } else {
            super::super::super::apply_route_strategy(candidates, key_id, model_for_log);
        }
        let account_dynamic_limits = super::super::super::rebalance_scheduler_candidates(
            storage,
            candidates,
            account_max_inflight,
            hint.pinned_account_id.is_some(),
        );
        let candidate_count = candidates.len();
        let candidate_order = candidates
            .iter()
            .map(|(account, _)| format!("{}#sort={}", account.id, account.sort))
            .collect::<Vec<_>>();
        super::super::super::trace_log::log_candidate_pool(
            trace_id,
            key_id,
            "peer_runtime",
            "peer_runtime",
            hint.pinned_account_id.is_some(),
            candidate_order.as_slice(),
        );
        return Ok(UpstreamRequestSetup {
            upstream_base,
            upstream_fallback_base,
            url,
            url_alt,
            candidate_count,
            account_max_inflight,
            account_dynamic_limits,
            anthropic_has_prompt_cache_key,
            has_sticky_fallback_session: false,
            has_sticky_fallback_conversation,
            has_body_encrypted_content,
            request_body_override: None,
            routing_state: RequestRoutingState::PeerRuntimeOnly,
            peer_runtime_key,
        });
    }

    super::super::super::apply_route_strategy(candidates, key_id, model_for_log);
    let account_dynamic_limits = super::super::super::rebalance_scheduler_candidates(
        storage,
        candidates,
        account_max_inflight,
        false,
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
        "route_strategy",
        true,
        candidate_order.as_slice(),
    );

    Ok(UpstreamRequestSetup {
        upstream_base,
        upstream_fallback_base,
        url,
        url_alt,
        candidate_count,
        account_max_inflight,
        account_dynamic_limits,
        anthropic_has_prompt_cache_key,
        has_sticky_fallback_session: false,
        has_sticky_fallback_conversation,
        has_body_encrypted_content,
        request_body_override: None,
        routing_state: RequestRoutingState::StatelessNoLegacy,
        peer_runtime_key,
    })
}

fn rotate_candidates_to_account(candidates: &mut [(Account, Token)], account_id: &str) -> bool {
    let Some(index) = candidates
        .iter()
        .position(|(account, _)| account.id == account_id)
    else {
        return false;
    };
    if index > 0 {
        candidates.rotate_left(index);
    }
    true
}
