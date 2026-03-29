use bytes::Bytes;
use codexmanager_core::storage::{
    now_ts, Account, AffinityScopePromotion, ClientBinding, ConversationContextEvent,
    ConversationContextState, ConversationThread, Storage, Token,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use crate::gateway::ResponseAdapter;

use super::{
    context_replay_enabled, current_affinity_soft_quota_percent, current_mode,
    current_replay_max_turns, derive_affinity_key, derive_thread_anchor, synthetic_scope_id,
    AffinityRoutingMode,
};

const RECENT_OUTCOME_WINDOW: usize = 32;
const BINDING_ACTIVE_TTL_SECS: i64 = 1_800;
const REPLAY_PAYLOAD_MAX_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct AffinityRoutingResolution {
    pub(crate) affinity_key: String,
    pub(crate) affinity_source: &'static str,
    pub(crate) conversation_scope_id: String,
    pub(crate) committed_conversation_scope_id: String,
    pub(crate) binding: Option<ClientBinding>,
    pub(crate) thread: Option<ConversationThread>,
    pub(crate) chosen_account_id: String,
    pub(crate) candidate_account_ids: Vec<String>,
    pub(crate) request_body_override: Option<Bytes>,
    pub(crate) thread_epoch: i64,
    pub(crate) thread_anchor: String,
    pub(crate) reset_session_affinity: bool,
    pub(crate) requires_replay: bool,
    pub(crate) current_turn_index: i64,
    pub(crate) primary_scope_id_for_commit: Option<String>,
    pub(crate) scope_promotion: Option<AffinityScopePromotion>,
    pub(crate) current_turn_input_items: Vec<Value>,
    pub(crate) selected_supply_score: Option<f64>,
    pub(crate) selected_pressure_score: Option<f64>,
    pub(crate) selected_final_score: Option<f64>,
    pub(crate) switch_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateState {
    Active,
    Draining,
    Exhausted,
    Cooldown,
    Unavailable,
}

#[derive(Debug, Clone)]
struct ScoredCandidate {
    account_id: String,
    supply_score: f64,
    pressure_score: f64,
    final_score: f64,
    deficit: i64,
    state: CandidateState,
    tie_break_index: usize,
}

#[derive(Default)]
struct AffinityRuntimeState {
    affinity_locks: HashMap<String, Arc<Mutex<()>>>,
    conversation_locks: HashMap<String, Arc<Mutex<()>>>,
    recent_outcomes: HashMap<String, VecDeque<bool>>,
    quota_faults: HashMap<String, VecDeque<i64>>,
}

static AFFINITY_RUNTIME_STATE: OnceLock<Mutex<AffinityRuntimeState>> = OnceLock::new();

fn runtime_state() -> &'static Mutex<AffinityRuntimeState> {
    AFFINITY_RUNTIME_STATE.get_or_init(|| Mutex::new(AffinityRuntimeState::default()))
}

pub(crate) fn acquire_affinity_lock(
    platform_key_hash: &str,
    affinity_key: &str,
) -> Arc<Mutex<()>> {
    let lock_key = format!("{platform_key_hash}:{affinity_key}");
    let mut state = crate::lock_utils::lock_recover(runtime_state(), "affinity_runtime_state");
    state
        .affinity_locks
        .entry(lock_key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

pub(crate) fn acquire_conversation_lock(
    platform_key_hash: &str,
    affinity_key: &str,
    conversation_scope_id: &str,
) -> Arc<Mutex<()>> {
    let lock_key = format!("{platform_key_hash}:{affinity_key}:{conversation_scope_id}");
    let mut state = crate::lock_utils::lock_recover(runtime_state(), "affinity_runtime_state");
    state
        .conversation_locks
        .entry(lock_key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

pub(crate) fn record_affinity_attempt_feedback(
    account_id: &str,
    status_code: u16,
    error: Option<&str>,
) {
    if status_code == 499 {
        return;
    }
    let now = now_ts();
    let mut state = crate::lock_utils::lock_recover(runtime_state(), "affinity_runtime_state");
    let outcomes = state
        .recent_outcomes
        .entry(account_id.to_string())
        .or_default();
    let is_success = (200..=299).contains(&status_code)
        && error
            .map(str::trim)
            .is_none_or(|value| value.is_empty());
    outcomes.push_back(is_success);
    while outcomes.len() > RECENT_OUTCOME_WINDOW {
        outcomes.pop_front();
    }

    let quota_faults = state.quota_faults.entry(account_id.to_string()).or_default();
    prune_quota_faults(quota_faults, now);
    if is_quota_like_429(status_code, error) {
        quota_faults.push_back(now);
        prune_quota_faults(quota_faults, now);
    } else if is_success {
        quota_faults.clear();
    }
}

fn is_quota_like_429(status_code: u16, error: Option<&str>) -> bool {
    if status_code != 429 {
        return false;
    }
    let normalized = error
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    let quota_markers = [
        "insufficient_quota",
        "billing_hard_limit",
        "usage_limit_reached",
        "monthly_quota_exceeded",
        "credit_balance_too_low",
        "quota",
        "billing",
        "credit balance",
    ];
    let non_quota_markers = [
        "rate limit",
        "too many requests",
        "retry later",
        "challenge",
        "cloudflare",
    ];
    quota_markers.iter().any(|marker| normalized.contains(marker))
        && !non_quota_markers
            .iter()
            .any(|marker| normalized.contains(marker))
}

pub(crate) fn resolve_enforced_routing(
    storage: &Storage,
    incoming_headers: &super::IncomingHeaderSnapshot,
    original_path: &str,
    path: &str,
    body: &Bytes,
    candidates: &mut Vec<(Account, Token)>,
    key_id: &str,
    platform_key_hash: &str,
    local_conversation_id: Option<&str>,
    model_for_log: Option<&str>,
    response_adapter: ResponseAdapter,
) -> Result<Option<AffinityRoutingResolution>, String> {
    if current_mode() != AffinityRoutingMode::Enforce {
        return Ok(None);
    }
    if !supports_affinity_persistence_request(original_path, path, response_adapter) {
        return Ok(None);
    }
    let Some(derived) = derive_affinity_key(incoming_headers, local_conversation_id) else {
        return Ok(None);
    };
    if candidates.is_empty() {
        return Err("no available account".to_string());
    }

    let request_context = parse_canonical_request_body(body.as_ref())
        .ok_or_else(|| "invalid canonical request for affinity context".to_string())?;
    let mut binding = storage
        .get_client_binding(platform_key_hash, derived.key.as_str())
        .map_err(|err| format!("load client binding failed: {err}"))?;
    let requested_conversation_id = normalize_text(local_conversation_id);
    let (
        conversation_scope_id,
        committed_conversation_scope_id,
        primary_scope_id_for_commit,
        scope_promotion,
    ) = resolve_scope_id(
        storage,
        platform_key_hash,
        derived.key.as_str(),
        binding.as_ref(),
        requested_conversation_id.as_deref(),
    )?;
    let mut thread = storage
        .get_conversation_thread(
            platform_key_hash,
            derived.key.as_str(),
            conversation_scope_id.as_str(),
        )
        .map_err(|err| format!("load conversation thread failed: {err}"))?;
    if binding.is_none() {
        if let Some(requested_conversation_id) = requested_conversation_id.as_deref() {
            if let Some(legacy_binding) = storage
                .get_conversation_binding(platform_key_hash, requested_conversation_id)
                .map_err(|err| format!("load legacy conversation binding failed: {err}"))?
            {
                binding = Some(ClientBinding {
                    platform_key_hash: platform_key_hash.to_string(),
                    affinity_key: derived.key.clone(),
                    account_id: legacy_binding.account_id.clone(),
                    primary_scope_id: Some(requested_conversation_id.to_string()),
                    binding_version: 0,
                    status: legacy_binding.status.clone(),
                    last_supply_score: None,
                    last_pressure_score: None,
                    last_final_score: None,
                    last_switch_reason: legacy_binding.last_switch_reason.clone(),
                    created_at: legacy_binding.created_at,
                    updated_at: legacy_binding.updated_at,
                    last_seen_at: legacy_binding.last_used_at,
                });
                if thread.is_none() {
                    thread = Some(ConversationThread {
                        platform_key_hash: platform_key_hash.to_string(),
                        affinity_key: derived.key.clone(),
                        conversation_scope_id: requested_conversation_id.to_string(),
                        account_id: legacy_binding.account_id,
                        thread_epoch: legacy_binding.thread_epoch,
                        thread_anchor: legacy_binding.thread_anchor,
                        thread_version: 0,
                        created_at: legacy_binding.created_at,
                        updated_at: legacy_binding.updated_at,
                        last_seen_at: legacy_binding.last_used_at,
                    });
                }
            }
        }
    }

    let tie_break_index = build_tie_break_index(candidates.as_slice(), key_id, model_for_log);
    let scored = score_candidates(
        storage,
        candidates.as_slice(),
        platform_key_hash,
        derived.key.as_str(),
        binding.as_ref(),
        tie_break_index,
    )?;
    let Some((chosen, candidate_account_ids, switch_reason)) = choose_target_candidates(
        candidates.as_slice(),
        binding.as_ref(),
        scored.as_slice(),
    ) else {
        return Err("no available account".to_string());
    };

    reorder_candidates(candidates, candidate_account_ids.as_slice());
    let selected_candidate = scored
        .iter()
        .find(|item| item.account_id == chosen)
        .ok_or_else(|| "selected candidate missing from score set".to_string())?;
    let requires_replay = thread
        .as_ref()
        .is_some_and(|item| item.account_id != chosen);
    let (thread_epoch, thread_anchor, reset_session_affinity) = resolve_thread_assignment(
        binding.as_ref(),
        thread.as_ref(),
        platform_key_hash,
        derived.key.as_str(),
        committed_conversation_scope_id.as_str(),
        requested_conversation_id.as_deref(),
        chosen.as_str(),
    );
    let request_body_override = if requires_replay {
        Some(Bytes::from(build_replay_request_body(
            storage,
            path,
            body.as_ref(),
            platform_key_hash,
            derived.key.as_str(),
            conversation_scope_id.as_str(),
        )?))
    } else {
        None
    };
    let current_turn_index = next_turn_index(
        storage,
        platform_key_hash,
        derived.key.as_str(),
        conversation_scope_id.as_str(),
    )?;

    Ok(Some(AffinityRoutingResolution {
        affinity_key: derived.key,
        affinity_source: derived.source,
        conversation_scope_id,
        committed_conversation_scope_id,
        binding,
        thread,
        chosen_account_id: chosen,
        candidate_account_ids,
        request_body_override,
        thread_epoch,
        thread_anchor,
        reset_session_affinity,
        requires_replay,
        current_turn_index,
        primary_scope_id_for_commit,
        scope_promotion,
        current_turn_input_items: request_context.input_items,
        selected_supply_score: Some(selected_candidate.supply_score),
        selected_pressure_score: Some(selected_candidate.pressure_score),
        selected_final_score: Some(selected_candidate.final_score),
        switch_reason,
    }))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn finalize_affinity_success(
    storage: &Storage,
    resolution: &AffinityRoutingResolution,
    platform_key_hash: &str,
    account_id: &str,
    request_body: &[u8],
    completed_response_body: Option<&[u8]>,
    response_adapter_label: &str,
    protocol_type: &str,
) -> Result<(), String> {
    if current_mode() != AffinityRoutingMode::Enforce {
        return Ok(());
    }

    let mut parsed_request = parse_canonical_request_body(request_body)
        .ok_or_else(|| "invalid canonical request for affinity context".to_string())?;
    let parsed_response = completed_response_body
        .and_then(parse_canonical_response_output_items)
        .ok_or_else(|| "missing canonical completed response for affinity context".to_string())?;
    if let Some(existing_state) = load_existing_context_state_for_commit(
        storage,
        platform_key_hash,
        resolution,
    )? {
        fill_missing_request_context_from_state(&mut parsed_request, &existing_state);
    }
    let now = now_ts();
    let switch_reason = if resolution.chosen_account_id == account_id {
        resolution.switch_reason.clone()
    } else {
        Some("affinity_probe_fallback".to_string())
    };
    let selected_supply_score = (resolution.chosen_account_id == account_id)
        .then_some(resolution.selected_supply_score)
        .flatten();
    let selected_pressure_score = (resolution.chosen_account_id == account_id)
        .then_some(resolution.selected_pressure_score)
        .flatten();
    let selected_final_score = (resolution.chosen_account_id == account_id)
        .then_some(resolution.selected_final_score)
        .flatten();
    let expected_binding_version = resolution.binding.as_ref().map(|item| item.binding_version);
    let binding = ClientBinding {
        platform_key_hash: platform_key_hash.to_string(),
        affinity_key: resolution.affinity_key.clone(),
        account_id: account_id.to_string(),
        primary_scope_id: resolution.primary_scope_id_for_commit.clone(),
        binding_version: expected_binding_version.unwrap_or(0) + 1,
        status: "active".to_string(),
        last_supply_score: selected_supply_score,
        last_pressure_score: selected_pressure_score,
        last_final_score: selected_final_score,
        last_switch_reason: switch_reason,
        created_at: resolution
            .binding
            .as_ref()
            .map(|item| item.created_at)
            .unwrap_or(now),
        updated_at: now,
        last_seen_at: now,
    };
    let expected_thread_version = resolution.thread.as_ref().map(|item| item.thread_version);
    let thread = ConversationThread {
        platform_key_hash: platform_key_hash.to_string(),
        affinity_key: resolution.affinity_key.clone(),
        conversation_scope_id: resolution.committed_conversation_scope_id.clone(),
        account_id: account_id.to_string(),
        thread_epoch: resolution.thread_epoch,
        thread_anchor: resolution.thread_anchor.clone(),
        thread_version: expected_thread_version.unwrap_or(0) + 1,
        created_at: resolution
            .thread
            .as_ref()
            .map(|item| item.created_at)
            .unwrap_or(now),
        updated_at: now,
        last_seen_at: now,
    };
    let context_state = ConversationContextState {
        platform_key_hash: platform_key_hash.to_string(),
        affinity_key: resolution.affinity_key.clone(),
        conversation_scope_id: resolution.committed_conversation_scope_id.clone(),
        model: parsed_request.model,
        instructions_text: parsed_request.instructions_text,
        tools_json: parsed_request.tools_json,
        tool_choice_json: parsed_request.tool_choice_json,
        parallel_tool_calls: parsed_request.parallel_tool_calls,
        reasoning_json: parsed_request.reasoning_json,
        text_format_json: parsed_request.text_format_json,
        service_tier: parsed_request.service_tier,
        metadata_json: parsed_request.metadata_json,
        encrypted_content: parsed_request.encrypted_content,
        protocol_type: Some(protocol_type.to_string()),
        response_adapter: Some(response_adapter_label.to_string()),
        updated_at: now,
    };
    let events = build_turn_events(
        platform_key_hash,
        resolution.affinity_key.as_str(),
        resolution.committed_conversation_scope_id.as_str(),
        resolution.current_turn_index,
        resolution.current_turn_input_items.as_slice(),
        parsed_response.as_slice(),
        now,
    )?;
    if events.is_empty() {
        log::warn!(
            "event=gateway_affinity_empty_turn_events affinity_key={} account_id={} request_items={} response_items={}",
            resolution.affinity_key,
            account_id,
            resolution.current_turn_input_items.len(),
            parsed_response.len(),
        );
    }
    let committed = storage
        .commit_affinity_turn_success(
            &binding,
            expected_binding_version,
            &thread,
            expected_thread_version,
            resolution.scope_promotion.as_ref(),
            &context_state,
            resolution.current_turn_index,
            events.as_slice(),
        )
        .map_err(|err| format!("commit affinity turn success failed: {err}"))?;
    if !committed {
        return Err("affinity_commit_conflict".to_string());
    }
    Ok(())
}

fn resolve_scope_id(
    storage: &Storage,
    platform_key_hash: &str,
    affinity_key: &str,
    binding: Option<&ClientBinding>,
    requested_conversation_id: Option<&str>,
) -> Result<
    (
        String,
        String,
        Option<String>,
        Option<AffinityScopePromotion>,
    ),
    String,
> {
    let Some(binding) = binding else {
        let scope_id = requested_conversation_id
            .map(str::to_string)
            .unwrap_or_else(|| synthetic_scope_id(platform_key_hash, affinity_key));
        return Ok((scope_id.clone(), scope_id.clone(), Some(scope_id), None));
    };

    let synthetic_primary = binding
        .primary_scope_id
        .as_deref()
        .is_some_and(|value| value.starts_with("affinity::"));
    if let Some(requested) = requested_conversation_id {
        if binding.primary_scope_id.as_deref() == Some(requested) {
            return Ok((
                requested.to_string(),
                requested.to_string(),
                Some(requested.to_string()),
                None,
            ));
        }
        if synthetic_primary {
            let synthetic_scope = binding.primary_scope_id.clone().unwrap_or_default();
            let existing_requested = storage
                .get_conversation_thread(platform_key_hash, affinity_key, requested)
                .map_err(|err| format!("load promoted conversation thread failed: {err}"))?;
            if existing_requested.is_none() {
                return Ok((
                    synthetic_scope.clone(),
                    requested.to_string(),
                    Some(requested.to_string()),
                    Some(AffinityScopePromotion {
                        platform_key_hash: platform_key_hash.to_string(),
                        affinity_key: affinity_key.to_string(),
                        from_scope_id: synthetic_scope,
                        to_scope_id: requested.to_string(),
                    }),
                ));
            }
        }
        return Ok((
            requested.to_string(),
            requested.to_string(),
            if synthetic_primary {
                Some(requested.to_string())
            } else {
                binding
                    .primary_scope_id
                    .clone()
                    .or_else(|| Some(requested.to_string()))
            },
            None,
        ));
    }

    let scope_id = binding
        .primary_scope_id
        .clone()
        .unwrap_or_else(|| synthetic_scope_id(platform_key_hash, affinity_key));
    Ok((scope_id.clone(), scope_id.clone(), Some(scope_id), None))
}

fn resolve_thread_assignment(
    binding: Option<&ClientBinding>,
    thread: Option<&ConversationThread>,
    platform_key_hash: &str,
    affinity_key: &str,
    conversation_scope_id: &str,
    requested_conversation_id: Option<&str>,
    chosen_account_id: &str,
) -> (i64, String, bool) {
    match thread {
        Some(thread) if thread.account_id == chosen_account_id => {
            (thread.thread_epoch, thread.thread_anchor.clone(), false)
        }
        Some(thread) => {
            let epoch = thread.thread_epoch + 1;
            (
                epoch,
                derive_thread_anchor(platform_key_hash, affinity_key, conversation_scope_id, epoch),
                true,
            )
        }
        None => {
            if binding.is_none() {
                if let Some(requested_conversation_id) = requested_conversation_id {
                    return (1, requested_conversation_id.to_string(), false);
                }
            }
            (
                1,
                requested_conversation_id
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        derive_thread_anchor(platform_key_hash, affinity_key, conversation_scope_id, 1)
                    }),
                false,
            )
        }
    }
}

fn build_tie_break_index(
    candidates: &[(Account, Token)],
    key_id: &str,
    model_for_log: Option<&str>,
) -> HashMap<String, usize> {
    let mut ordered = candidates.to_vec();
    super::super::apply_route_strategy(&mut ordered, key_id, model_for_log);
    ordered
        .iter()
        .enumerate()
        .map(|(idx, (account, _))| (account.id.clone(), idx))
        .collect()
}

fn score_candidates(
    storage: &Storage,
    candidates: &[(Account, Token)],
    platform_key_hash: &str,
    affinity_key: &str,
    binding: Option<&ClientBinding>,
    tie_break_index: HashMap<String, usize>,
) -> Result<Vec<ScoredCandidate>, String> {
    let exclude_key = binding.map(|_| (platform_key_hash, affinity_key));
    let recent_cutoff = now_ts().saturating_sub(BINDING_ACTIVE_TTL_SECS);
    let mut base = Vec::with_capacity(candidates.len());
    let mut total_supply = 0.0_f64;
    let mut total_effective_bindings = 0_i64;
    for (account, token) in candidates {
        let snapshot =
            super::super::scheduler::account_runtime_snapshot(storage, account.id.as_str(), token, 0);
        let state = evaluate_candidate_state(account, token, &snapshot);
        let quota_ratio = if snapshot.usage_known && snapshot.usage_snapshot_fresh {
            (snapshot.remaining_quota_percent / 100.0).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let pass_prob_recent32 = pass_probability_recent32(account.id.as_str());
        let route_health_norm = (f64::from(snapshot.route_health_score.clamp(0, 200)) / 200.0)
            .clamp(0.0, 1.0);
        let headroom = if snapshot.dynamic_limit == 0 {
            1.0
        } else {
            (snapshot.dynamic_limit.saturating_sub(snapshot.inflight) as f64
                / snapshot.dynamic_limit.max(1) as f64)
                .clamp(0.0, 1.0)
        };
        let latency_score = snapshot
            .latency_ewma_ms
            .map(|value| (1.0 / (1.0 + value / 2_500.0)).clamp(0.0, 1.0))
            .unwrap_or(0.7);
        // 中文注释：额度是硬供给上限，必须作为主尺度；否则健康信号会把低额度账号错误抬高。
        let quality_score = 0.40 * pass_prob_recent32
            + 0.25 * route_health_norm
            + 0.20 * headroom
            + 0.15 * latency_score;
        let supply = quota_ratio * quality_score;
        let effective_bindings = storage
            .count_recent_client_bindings_for_account(account.id.as_str(), recent_cutoff, exclude_key)
            .map_err(|err| format!("count recent client bindings failed: {err}"))?;
        total_effective_bindings += effective_bindings;
        if state == CandidateState::Active {
            total_supply += supply.max(0.0);
        }
        base.push((
            account.id.clone(),
            supply.max(0.0),
            effective_bindings,
            state,
            tie_break_index
                .get(account.id.as_str())
                .copied()
                .unwrap_or(usize::MAX),
        ));
    }

    let projected_total = total_effective_bindings + 1;
    let active_candidates = base
        .iter()
        .filter(|(_, _, _, state, _)| *state == CandidateState::Active)
        .map(|(account_id, supply, _, _, tie_break)| (account_id.clone(), *supply, *tie_break))
        .collect::<Vec<_>>();
    let targets = hamilton_targets(active_candidates.as_slice(), total_supply, projected_total);
    let manual_preferred = super::super::manual_preferred_account();

    Ok(base
        .into_iter()
        .map(|(account_id, supply, effective_bindings, state, tie_break_index)| {
            let target_bindings = targets.get(account_id.as_str()).copied().unwrap_or(0);
            let pressure_score = if target_bindings <= 0 || effective_bindings <= target_bindings {
                1.0
            } else {
                let pressure = effective_bindings as f64 / target_bindings.max(1) as f64;
                (1.0 / (1.0 + 0.85 * (pressure - 1.0))).clamp(0.0, 1.0)
            };
            let mut final_score = if state == CandidateState::Active {
                supply * pressure_score
            } else {
                0.0
            };
            if manual_preferred
                .as_deref()
                .is_some_and(|preferred| preferred == account_id)
                && state == CandidateState::Active
            {
                final_score += 0.08;
            }
            ScoredCandidate {
                account_id,
                supply_score: supply,
                pressure_score,
                final_score,
                deficit: (target_bindings - effective_bindings).max(0),
                state,
                tie_break_index,
            }
        })
        .collect())
}

fn hamilton_targets(
    candidates: &[(String, f64, usize)],
    total_supply: f64,
    projected_total: i64,
) -> HashMap<String, i64> {
    if candidates.is_empty() || projected_total <= 0 {
        return HashMap::new();
    }
    let denominator = if total_supply > 0.0 {
        total_supply
    } else {
        candidates.len() as f64
    };
    let mut floor_sum = 0_i64;
    let mut raw = candidates
        .iter()
        .map(|(account_id, supply, tie_break)| {
            let normalized_supply = if total_supply > 0.0 { *supply } else { 1.0 };
            let exact = (normalized_supply / denominator) * projected_total as f64;
            let floor_value = exact.floor() as i64;
            floor_sum += floor_value;
            (
                account_id.clone(),
                floor_value,
                exact - floor_value as f64,
                *tie_break,
            )
        })
        .collect::<Vec<_>>();
    let mut remaining = projected_total.saturating_sub(floor_sum);
    raw.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.3.cmp(&b.3))
    });
    let mut targets = raw
        .iter()
        .map(|(account_id, floor_value, _, _)| (account_id.clone(), *floor_value))
        .collect::<HashMap<_, _>>();
    for (account_id, _, _, _) in raw {
        if remaining <= 0 {
            break;
        }
        if let Some(target) = targets.get_mut(account_id.as_str()) {
            *target += 1;
            remaining -= 1;
        }
    }
    targets
}

fn choose_target_candidates(
    _candidates: &[(Account, Token)],
    binding: Option<&ClientBinding>,
    scored: &[ScoredCandidate],
) -> Option<(String, Vec<String>, Option<String>)> {
    let mut active = scored
        .iter()
        .filter(|item| item.state == CandidateState::Active)
        .cloned()
        .collect::<Vec<_>>();
    active.sort_by(compare_scored_candidates);

    let bound = binding
        .and_then(|item| scored.iter().find(|candidate| candidate.account_id == item.account_id))
        .cloned();
    if let Some(bound) = bound.as_ref() {
        match bound.state {
            CandidateState::Active => {
                return Some((bound.account_id.clone(), vec![bound.account_id.clone()], None));
            }
            CandidateState::Draining if active.is_empty() => {
                return Some((bound.account_id.clone(), vec![bound.account_id.clone()], None));
            }
            CandidateState::Draining if !active.is_empty() => {
                let next = active[0].account_id.clone();
                let mut accounts = vec![next.clone()];
                if let Some(second) = active.get(1) {
                    accounts.push(second.account_id.clone());
                }
                return Some((next, accounts, Some("soft_quota_drain".to_string())));
            }
            CandidateState::Cooldown if active.is_empty() => {
                return Some((bound.account_id.clone(), vec![bound.account_id.clone()], None));
            }
            _ => {}
        }
    }

    if let Some(first) = active.first() {
        let mut accounts = vec![first.account_id.clone()];
        if let Some(second) = active.get(1) {
            accounts.push(second.account_id.clone());
        }
        let reason = binding
            .and_then(|item| (item.account_id != first.account_id).then_some("affinity_rebind"))
            .map(str::to_string);
        return Some((first.account_id.clone(), accounts, reason));
    }

    let mut fallback = scored
        .iter()
        .filter(|item| matches!(item.state, CandidateState::Draining))
        .cloned()
        .collect::<Vec<_>>();
    fallback.sort_by(compare_scored_candidates);
    fallback.first().map(|first| {
        (
            first.account_id.clone(),
            vec![first.account_id.clone()],
            binding
                .and_then(|item| (item.account_id != first.account_id).then_some("affinity_fallback"))
                .map(str::to_string),
        )
    })
}

fn compare_scored_candidates(left: &ScoredCandidate, right: &ScoredCandidate) -> std::cmp::Ordering {
    right
        .deficit
        .cmp(&left.deficit)
        .then_with(|| {
            right
                .final_score
                .partial_cmp(&left.final_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .then_with(|| left.tie_break_index.cmp(&right.tie_break_index))
}

fn reorder_candidates(candidates: &mut Vec<(Account, Token)>, ordered_account_ids: &[String]) {
    let mut by_account = candidates
        .drain(..)
        .map(|item| (item.0.id.clone(), item))
        .collect::<HashMap<_, _>>();
    let mut reordered = Vec::with_capacity(by_account.len());
    for account_id in ordered_account_ids {
        if let Some(item) = by_account.remove(account_id) {
            reordered.push(item);
        }
    }
    let mut remaining = by_account.into_values().collect::<Vec<_>>();
    remaining.sort_by(|left, right| {
        left.0
            .sort
            .cmp(&right.0.sort)
            .then_with(|| left.0.updated_at.cmp(&right.0.updated_at))
    });
    reordered.extend(remaining);
    *candidates = reordered;
}

fn evaluate_candidate_state(
    account: &Account,
    token: &Token,
    snapshot: &super::super::scheduler::SchedulerAccountSnapshot,
) -> CandidateState {
    if !account.status.eq_ignore_ascii_case("active")
        || token.access_token.trim().is_empty()
        || token.refresh_token.trim().is_empty()
    {
        return CandidateState::Unavailable;
    }
    let quota_faults = quota_fault_count(account.id.as_str());
    if snapshot.usage_snapshot_fresh && snapshot.remaining_quota_percent <= 0.0 {
        return CandidateState::Exhausted;
    }
    if quota_faults >= 2 {
        return CandidateState::Exhausted;
    }
    if snapshot.cooldown_until.is_some_and(|value| value > now_ts()) {
        return CandidateState::Cooldown;
    }
    let soft_quota = current_affinity_soft_quota_percent() as f64;
    if (snapshot.usage_snapshot_fresh
        && snapshot.remaining_quota_percent <= soft_quota
        && snapshot.remaining_quota_percent > 0.0)
        || quota_faults == 1
    {
        return CandidateState::Draining;
    }
    CandidateState::Active
}

fn pass_probability_recent32(account_id: &str) -> f64 {
    let state = crate::lock_utils::lock_recover(runtime_state(), "affinity_runtime_state");
    let Some(outcomes) = state.recent_outcomes.get(account_id) else {
        return 0.5;
    };
    let success_count = outcomes.iter().filter(|value| **value).count() as f64;
    let fail_count = outcomes.iter().filter(|value| !**value).count() as f64;
    (success_count + 2.0) / (success_count + fail_count + 4.0)
}

fn quota_fault_count(account_id: &str) -> usize {
    let mut state = crate::lock_utils::lock_recover(runtime_state(), "affinity_runtime_state");
    let Some(queue) = state.quota_faults.get_mut(account_id) else {
        return 0;
    };
    prune_quota_faults(queue, now_ts());
    queue.len()
}

fn prune_quota_faults(queue: &mut VecDeque<i64>, now: i64) {
    while queue.front().is_some_and(|value| now.saturating_sub(*value) > 60) {
        queue.pop_front();
    }
    while queue.len() > 2 {
        queue.pop_front();
    }
}

fn next_turn_index(
    storage: &Storage,
    platform_key_hash: &str,
    affinity_key: &str,
    conversation_scope_id: &str,
) -> Result<i64, String> {
    let events = storage
        .list_conversation_context_events(platform_key_hash, affinity_key, conversation_scope_id)
        .map_err(|err| format!("load context events for turn index failed: {err}"))?;
    Ok(events.iter().map(|item| item.turn_index).max().unwrap_or(0) + 1)
}

fn supports_affinity_persistence_request(
    original_path: &str,
    adapted_path: &str,
    response_adapter: ResponseAdapter,
) -> bool {
    response_adapter == ResponseAdapter::Passthrough
        && original_path.starts_with("/v1/responses")
        && adapted_path.starts_with("/v1/responses")
}

#[derive(Default)]
#[derive(Debug, Clone)]
struct ParsedRequestContext {
    model: Option<String>,
    instructions_text: Option<String>,
    tools_json: Option<String>,
    tool_choice_json: Option<String>,
    parallel_tool_calls: Option<bool>,
    reasoning_json: Option<String>,
    text_format_json: Option<String>,
    service_tier: Option<String>,
    metadata_json: Option<String>,
    encrypted_content: Option<String>,
    input_items: Vec<Value>,
}

fn parse_canonical_request_body(body: &[u8]) -> Option<ParsedRequestContext> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    let object = value.as_object()?;
    let input_items = normalize_input_items(object.get("input"))?;
    Some(ParsedRequestContext {
        model: object.get("model").and_then(Value::as_str).map(str::to_string),
        instructions_text: object
            .get("instructions")
            .and_then(Value::as_str)
            .map(str::to_string),
        tools_json: object.get("tools").and_then(|item| serde_json::to_string(item).ok()),
        tool_choice_json: object
            .get("tool_choice")
            .and_then(|item| serde_json::to_string(item).ok()),
        parallel_tool_calls: object.get("parallel_tool_calls").and_then(Value::as_bool),
        reasoning_json: object
            .get("reasoning")
            .and_then(|item| serde_json::to_string(item).ok()),
        text_format_json: object.get("text").and_then(|item| serde_json::to_string(item).ok()),
        service_tier: object
            .get("service_tier")
            .and_then(Value::as_str)
            .map(str::to_string),
        metadata_json: object
            .get("metadata")
            .and_then(|item| serde_json::to_string(item).ok()),
        encrypted_content: object
            .get("encrypted_content")
            .and_then(|item| serde_json::to_string(item).ok()),
        input_items,
    })
}

fn parse_canonical_response_output_items(body: &[u8]) -> Option<Vec<Value>> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    value.get("output")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| {
            value
                .get("response")
                .and_then(|item| item.get("output"))
                .and_then(Value::as_array)
                .cloned()
        })
}

fn normalize_input_items(input: Option<&Value>) -> Option<Vec<Value>> {
    match input {
        Some(Value::Array(items)) => Some(items.clone()),
        Some(value) => Some(vec![value.clone()]),
        None => Some(Vec::new()),
    }
}

fn build_replay_request_body(
    storage: &Storage,
    path: &str,
    body: &[u8],
    platform_key_hash: &str,
    affinity_key: &str,
    conversation_scope_id: &str,
) -> Result<Vec<u8>, String> {
    if !context_replay_enabled() || !path.starts_with("/v1/responses") {
        return Err("affinity_migration_context_unavailable".to_string());
    }
    let mut request_value =
        serde_json::from_slice::<Value>(body).map_err(|_| "invalid replay request body".to_string())?;
    let request_object = request_value
        .as_object_mut()
        .ok_or_else(|| "invalid replay request object".to_string())?;
    let current_items = normalize_input_items(request_object.get("input"))
        .ok_or_else(|| "missing replay request input".to_string())?;
    let state = storage
        .get_conversation_context_state(platform_key_hash, affinity_key, conversation_scope_id)
        .map_err(|err| format!("load conversation context state failed: {err}"))?
        .ok_or_else(|| "affinity_migration_context_unavailable".to_string())?;
    let events = storage
        .list_conversation_context_events(platform_key_hash, affinity_key, conversation_scope_id)
        .map_err(|err| format!("load conversation context events failed: {err}"))?;
    let replay_items = trim_replay_items(events)?;
    fill_missing_top_level_fields(request_object, &state);
    let mut merged = replay_items;
    merged.extend(current_items);
    request_object.insert("input".to_string(), Value::Array(merged));
    let bytes = serde_json::to_vec(&request_value)
        .map_err(|err| format!("serialize replay request failed: {err}"))?;
    if bytes.len() > REPLAY_PAYLOAD_MAX_BYTES {
        return Err("affinity_migration_context_unavailable".to_string());
    }
    Ok(bytes)
}

fn trim_replay_items(events: Vec<ConversationContextEvent>) -> Result<Vec<Value>, String> {
    let replay_max_turns = current_replay_max_turns() as usize;
    let mut by_turn = BTreeMap::<i64, Vec<ConversationContextEvent>>::new();
    for event in events {
        by_turn.entry(event.turn_index).or_default().push(event);
    }
    while by_turn.len() > replay_max_turns {
        let Some(oldest_turn) = by_turn.keys().next().copied() else {
            break;
        };
        let dropped = by_turn.remove(&oldest_turn).unwrap_or_default();
        if dropped.iter().any(is_tool_pair_event) {
            return Err("affinity_migration_context_unavailable".to_string());
        }
    }
    let mut items = Vec::new();
    for (_, mut turn_events) in by_turn {
        turn_events.sort_by_key(|event| event.item_seq);
        for event in turn_events {
            let value = serde_json::from_str::<Value>(&event.item_json)
                .map_err(|err| format!("parse replay item failed: {err}"))?;
            items.push(value);
        }
    }
    Ok(items)
}

fn fill_missing_top_level_fields(
    request_object: &mut serde_json::Map<String, Value>,
    state: &ConversationContextState,
) {
    if request_object.get("model").is_none() {
        if let Some(model) = state.model.as_ref() {
            request_object.insert("model".to_string(), Value::String(model.clone()));
        }
    }
    if request_object.get("instructions").is_none() {
        if let Some(instructions) = state.instructions_text.as_ref() {
            request_object.insert("instructions".to_string(), Value::String(instructions.clone()));
        }
    }
    if request_object.get("tools").is_none() {
        if let Some(tools_json) = state.tools_json.as_ref() {
            if let Ok(value) = serde_json::from_str::<Value>(tools_json) {
                request_object.insert("tools".to_string(), value);
            }
        }
    }
    if request_object.get("tool_choice").is_none() {
        if let Some(tool_choice_json) = state.tool_choice_json.as_ref() {
            if let Ok(value) = serde_json::from_str::<Value>(tool_choice_json) {
                request_object.insert("tool_choice".to_string(), value);
            }
        }
    }
    if request_object.get("parallel_tool_calls").is_none() {
        if let Some(parallel_tool_calls) = state.parallel_tool_calls {
            request_object.insert("parallel_tool_calls".to_string(), Value::Bool(parallel_tool_calls));
        }
    }
    if request_object.get("reasoning").is_none() {
        if let Some(reasoning_json) = state.reasoning_json.as_ref() {
            if let Ok(value) = serde_json::from_str::<Value>(reasoning_json) {
                request_object.insert("reasoning".to_string(), value);
            }
        }
    }
    if request_object.get("text").is_none() {
        if let Some(text_format_json) = state.text_format_json.as_ref() {
            if let Ok(value) = serde_json::from_str::<Value>(text_format_json) {
                request_object.insert("text".to_string(), value);
            }
        }
    }
    if request_object.get("service_tier").is_none() {
        if let Some(service_tier) = state.service_tier.as_ref() {
            request_object.insert("service_tier".to_string(), Value::String(service_tier.clone()));
        }
    }
    if request_object.get("metadata").is_none() {
        if let Some(metadata_json) = state.metadata_json.as_ref() {
            if let Ok(value) = serde_json::from_str::<Value>(metadata_json) {
                request_object.insert("metadata".to_string(), value);
            }
        }
    }
    if request_object.get("encrypted_content").is_none() {
        if let Some(encrypted_content) = state.encrypted_content.as_ref() {
            if let Ok(value) = serde_json::from_str::<Value>(encrypted_content) {
                request_object.insert("encrypted_content".to_string(), value);
            }
        }
    }
}

fn fill_missing_request_context_from_state(
    request_context: &mut ParsedRequestContext,
    state: &ConversationContextState,
) {
    if request_context.model.is_none() {
        request_context.model = state.model.clone();
    }
    if request_context.instructions_text.is_none() {
        request_context.instructions_text = state.instructions_text.clone();
    }
    if request_context.tools_json.is_none() {
        request_context.tools_json = state.tools_json.clone();
    }
    if request_context.tool_choice_json.is_none() {
        request_context.tool_choice_json = state.tool_choice_json.clone();
    }
    if request_context.parallel_tool_calls.is_none() {
        request_context.parallel_tool_calls = state.parallel_tool_calls;
    }
    if request_context.reasoning_json.is_none() {
        request_context.reasoning_json = state.reasoning_json.clone();
    }
    if request_context.text_format_json.is_none() {
        request_context.text_format_json = state.text_format_json.clone();
    }
    if request_context.service_tier.is_none() {
        request_context.service_tier = state.service_tier.clone();
    }
    if request_context.metadata_json.is_none() {
        request_context.metadata_json = state.metadata_json.clone();
    }
    if request_context.encrypted_content.is_none() {
        request_context.encrypted_content = state.encrypted_content.clone();
    }
}

fn load_existing_context_state_for_commit(
    storage: &Storage,
    platform_key_hash: &str,
    resolution: &AffinityRoutingResolution,
) -> Result<Option<ConversationContextState>, String> {
    if let Some(existing_state) = storage
        .get_conversation_context_state(
            platform_key_hash,
            resolution.affinity_key.as_str(),
            resolution.committed_conversation_scope_id.as_str(),
        )
        .map_err(|err| format!("load existing conversation context state failed: {err}"))?
    {
        return Ok(Some(existing_state));
    }
    if let Some(promotion) = resolution.scope_promotion.as_ref() {
        if promotion.from_scope_id != promotion.to_scope_id {
            return storage
                .get_conversation_context_state(
                    platform_key_hash,
                    resolution.affinity_key.as_str(),
                    promotion.from_scope_id.as_str(),
                )
                .map_err(|err| format!("load promoted source context state failed: {err}"));
        }
    }
    Ok(None)
}

fn build_turn_events(
    platform_key_hash: &str,
    affinity_key: &str,
    conversation_scope_id: &str,
    turn_index: i64,
    request_items: &[Value],
    response_output_items: &[Value],
    created_at: i64,
) -> Result<Vec<ConversationContextEvent>, String> {
    let mut events = Vec::new();
    for (idx, item) in request_items.iter().enumerate() {
        events.push(build_context_event(
            platform_key_hash,
            affinity_key,
            conversation_scope_id,
            turn_index,
            idx as i64,
            item,
            created_at,
        )?);
    }
    let offset = request_items.len() as i64;
    for (idx, item) in response_output_items.iter().enumerate() {
        events.push(build_context_event(
            platform_key_hash,
            affinity_key,
            conversation_scope_id,
            turn_index,
            offset + idx as i64,
            item,
            created_at,
        )?);
    }
    Ok(events)
}

fn build_context_event(
    platform_key_hash: &str,
    affinity_key: &str,
    conversation_scope_id: &str,
    turn_index: i64,
    item_seq: i64,
    item: &Value,
    created_at: i64,
) -> Result<ConversationContextEvent, String> {
    let role = item.get("role").and_then(Value::as_str).map(str::to_string);
    let pair_group_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| item.get("id").and_then(Value::as_str).map(str::to_string));
    let item_json = serde_json::to_string(item)
        .map_err(|err| format!("serialize context event failed: {err}"))?;
    Ok(ConversationContextEvent {
        platform_key_hash: platform_key_hash.to_string(),
        affinity_key: affinity_key.to_string(),
        conversation_scope_id: conversation_scope_id.to_string(),
        turn_index,
        item_seq,
        role,
        pair_group_id,
        capture_complete: true,
        item_json,
        created_at,
    })
}

fn is_tool_pair_event(event: &ConversationContextEvent) -> bool {
    serde_json::from_str::<Value>(&event.item_json)
        .ok()
        .and_then(|value| value.get("type").and_then(Value::as_str).map(str::to_string))
        .is_some_and(|kind| kind == "function_call" || kind == "function_call_output")
}

fn normalize_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::{
        build_replay_request_body, compare_scored_candidates, evaluate_candidate_state,
        hamilton_targets, now_ts, record_affinity_attempt_feedback, resolve_scope_id,
        supports_affinity_persistence_request, trim_replay_items, Account,
        AffinityScopePromotion, ClientBinding, ConversationContextEvent,
        ConversationContextState, ConversationThread, ScoredCandidate, Storage, Token,
    };
    use bytes::Bytes;
    use crate::gateway::ResponseAdapter;
    use std::sync::{Mutex, OnceLock};

    fn affinity_runtime_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn hamilton_targets_assigns_full_total() {
        let targets = hamilton_targets(
            &[("acc-1".to_string(), 0.8, 0), ("acc-2".to_string(), 0.2, 1)],
            1.0,
            5,
        );
        assert_eq!(targets.values().sum::<i64>(), 5);
        assert_eq!(targets.get("acc-1").copied(), Some(4));
        assert_eq!(targets.get("acc-2").copied(), Some(1));
    }

    #[test]
    fn compare_scored_candidates_prefers_deficit_then_score() {
        let left = ScoredCandidate {
            account_id: "acc-1".to_string(),
            supply_score: 0.8,
            pressure_score: 1.0,
            final_score: 0.8,
            deficit: 1,
            state: super::CandidateState::Active,
            tie_break_index: 0,
        };
        let right = ScoredCandidate {
            account_id: "acc-2".to_string(),
            supply_score: 0.9,
            pressure_score: 1.0,
            final_score: 0.9,
            deficit: 0,
            state: super::CandidateState::Active,
            tie_break_index: 1,
        };
        assert_eq!(compare_scored_candidates(&left, &right), std::cmp::Ordering::Less);
    }

    #[test]
    fn trim_replay_items_rejects_tool_pair_turns_when_window_would_drop_them() {
        let _guard = affinity_runtime_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_replay_max_turns = crate::gateway::affinity::current_replay_max_turns();
        crate::gateway::affinity::set_replay_max_turns(1).expect("set replay max turns");

        let err = trim_replay_items(vec![
            ConversationContextEvent {
                platform_key_hash: "pk".to_string(),
                affinity_key: "sid:test".to_string(),
                conversation_scope_id: "scope".to_string(),
                turn_index: 0,
                item_seq: 0,
                role: Some("assistant".to_string()),
                pair_group_id: Some("pair-1".to_string()),
                capture_complete: true,
                item_json: "{\"type\":\"function_call\"}".to_string(),
                created_at: 1,
            },
            ConversationContextEvent {
                platform_key_hash: "pk".to_string(),
                affinity_key: "sid:test".to_string(),
                conversation_scope_id: "scope".to_string(),
                turn_index: 1,
                item_seq: 0,
                role: Some("user".to_string()),
                pair_group_id: None,
                capture_complete: true,
                item_json: "{\"type\":\"message\",\"role\":\"user\"}".to_string(),
                created_at: 2,
            },
        ])
        .expect_err("tool-pair turn should block lossy trimming");
        crate::gateway::affinity::set_replay_max_turns(previous_replay_max_turns)
            .expect("restore replay max turns");

        assert_eq!(err, "affinity_migration_context_unavailable");
    }

    #[test]
    fn build_replay_request_body_merges_history_and_missing_top_level_fields() {
        let _guard = affinity_runtime_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_context_replay = crate::gateway::affinity::context_replay_enabled();
        let previous_replay_max_turns = crate::gateway::affinity::current_replay_max_turns();
        crate::gateway::affinity::set_context_replay_enabled(true);
        crate::gateway::affinity::set_replay_max_turns(4).expect("set replay max turns");

        let storage = Storage::open_in_memory().expect("open in memory");
        storage.init().expect("init schema");
        storage
            .save_conversation_context_state(&ConversationContextState {
                platform_key_hash: "pk".to_string(),
                affinity_key: "sid:test".to_string(),
                conversation_scope_id: "scope".to_string(),
                model: Some("gpt-5.4".to_string()),
                instructions_text: Some("be precise".to_string()),
                tools_json: Some("[{\"type\":\"function\",\"name\":\"lookup\"}]".to_string()),
                tool_choice_json: Some("{\"type\":\"auto\"}".to_string()),
                parallel_tool_calls: Some(true),
                reasoning_json: None,
                text_format_json: None,
                service_tier: Some("default".to_string()),
                metadata_json: Some("{\"source\":\"test\"}".to_string()),
                encrypted_content: None,
                protocol_type: Some("openai_compat".to_string()),
                response_adapter: Some("Passthrough".to_string()),
                updated_at: 1,
            })
            .expect("save state");
        storage
            .replace_conversation_context_turn(
                "pk",
                "sid:test",
                "scope",
                0,
                &[ConversationContextEvent {
                    platform_key_hash: "pk".to_string(),
                    affinity_key: "sid:test".to_string(),
                    conversation_scope_id: "scope".to_string(),
                    turn_index: 0,
                    item_seq: 0,
                    role: Some("assistant".to_string()),
                    pair_group_id: None,
                    capture_complete: true,
                    item_json: "{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"history\"}]}".to_string(),
                    created_at: 1,
                }],
            )
            .expect("save turn");

        let replay = build_replay_request_body(
            &storage,
            "/v1/responses",
            br#"{"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"next"}]}]}"#,
            "pk",
            "sid:test",
            "scope",
        )
        .expect("build replay body");

        let payload: serde_json::Value =
            serde_json::from_slice(&replay).expect("parse replay body");
        crate::gateway::affinity::set_context_replay_enabled(previous_context_replay);
        crate::gateway::affinity::set_replay_max_turns(previous_replay_max_turns)
            .expect("restore replay max turns");
        assert_eq!(
            payload.get("model").and_then(serde_json::Value::as_str),
            Some("gpt-5.4")
        );
        assert!(payload.get("tools").is_some());
        assert_eq!(
            payload
                .get("parallel_tool_calls")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
        let input = payload
            .get("input")
            .and_then(serde_json::Value::as_array)
            .expect("input array");
        assert_eq!(input.len(), 2);
        assert_eq!(
            input[0].get("role").and_then(serde_json::Value::as_str),
            Some("assistant")
        );
        assert_eq!(
            input[1].get("role").and_then(serde_json::Value::as_str),
            Some("user")
        );
    }

    #[test]
    fn finalize_affinity_success_preserves_existing_top_level_state_when_request_omits_it() {
        let _guard = affinity_runtime_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_mode = crate::gateway::affinity::current_mode();
        crate::gateway::affinity::set_mode("enforce").expect("set enforce mode");

        let storage = Storage::open_in_memory().expect("open in memory");
        storage.init().expect("init schema");
        storage
            .save_conversation_context_state(&ConversationContextState {
                platform_key_hash: "pk".to_string(),
                affinity_key: "sid:preserve".to_string(),
                conversation_scope_id: "scope".to_string(),
                model: Some("gpt-5.4".to_string()),
                instructions_text: Some("stay strict".to_string()),
                tools_json: Some("[{\"type\":\"function\",\"name\":\"lookup\"}]".to_string()),
                tool_choice_json: Some("{\"type\":\"auto\"}".to_string()),
                parallel_tool_calls: Some(true),
                reasoning_json: Some("{\"effort\":\"medium\"}".to_string()),
                text_format_json: Some("{\"format\":\"text\"}".to_string()),
                service_tier: Some("default".to_string()),
                metadata_json: Some("{\"source\":\"saved\"}".to_string()),
                encrypted_content: Some("{\"cipher\":\"x\"}".to_string()),
                protocol_type: Some("openai_compat".to_string()),
                response_adapter: Some("Passthrough".to_string()),
                updated_at: 1,
            })
            .expect("seed state");
        let resolution = super::AffinityRoutingResolution {
            affinity_key: "sid:preserve".to_string(),
            affinity_source: "session_id",
            conversation_scope_id: "scope".to_string(),
            committed_conversation_scope_id: "scope".to_string(),
            binding: None,
            thread: None,
            chosen_account_id: "acc-1".to_string(),
            candidate_account_ids: vec!["acc-1".to_string()],
            request_body_override: None,
            thread_epoch: 1,
            thread_anchor: "thread-1".to_string(),
            reset_session_affinity: false,
            requires_replay: false,
            current_turn_index: 1,
            primary_scope_id_for_commit: Some("scope".to_string()),
            scope_promotion: None,
            current_turn_input_items: vec![serde_json::json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "next"}]
            })],
            selected_supply_score: Some(0.9),
            selected_pressure_score: Some(1.0),
            selected_final_score: Some(0.9),
            switch_reason: Some("steady_state".to_string()),
        };

        super::finalize_affinity_success(
            &storage,
            &resolution,
            "pk",
            "acc-1",
            br#"{"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"next"}]}]}"#,
            Some(
                br#"{"response":{"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]}]}}"#,
            ),
            "Passthrough",
            "openai_compat",
        )
        .expect("finalize success");

        let state = storage
            .get_conversation_context_state("pk", "sid:preserve", "scope")
            .expect("load state")
            .expect("state exists");
        assert_eq!(state.instructions_text.as_deref(), Some("stay strict"));
        assert_eq!(
            state.tools_json.as_deref(),
            Some("[{\"type\":\"function\",\"name\":\"lookup\"}]")
        );
        assert_eq!(state.parallel_tool_calls, Some(true));
        assert_eq!(state.metadata_json.as_deref(), Some("{\"source\":\"saved\"}"));

        crate::gateway::affinity::set_mode(previous_mode.as_str()).expect("restore mode");
    }

    #[test]
    fn finalize_affinity_success_preserves_promoted_scope_state_when_request_omits_it() {
        let _guard = affinity_runtime_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_mode = crate::gateway::affinity::current_mode();
        crate::gateway::affinity::set_mode("enforce").expect("set enforce mode");

        let storage = Storage::open_in_memory().expect("open in memory");
        storage.init().expect("init schema");
        storage
            .save_conversation_context_state(&ConversationContextState {
                platform_key_hash: "pk".to_string(),
                affinity_key: "sid:promote".to_string(),
                conversation_scope_id: "affinity::synthetic".to_string(),
                model: Some("gpt-5.4".to_string()),
                instructions_text: Some("carry synthetic".to_string()),
                tools_json: Some("[{\"type\":\"function\",\"name\":\"lookup\"}]".to_string()),
                tool_choice_json: Some("{\"type\":\"auto\"}".to_string()),
                parallel_tool_calls: Some(true),
                reasoning_json: None,
                text_format_json: None,
                service_tier: None,
                metadata_json: Some("{\"scope\":\"synthetic\"}".to_string()),
                encrypted_content: None,
                protocol_type: Some("openai_compat".to_string()),
                response_adapter: Some("Passthrough".to_string()),
                updated_at: 1,
            })
            .expect("seed synthetic state");
        let resolution = super::AffinityRoutingResolution {
            affinity_key: "sid:promote".to_string(),
            affinity_source: "session_id",
            conversation_scope_id: "affinity::synthetic".to_string(),
            committed_conversation_scope_id: "conv-real".to_string(),
            binding: None,
            thread: None,
            chosen_account_id: "acc-1".to_string(),
            candidate_account_ids: vec!["acc-1".to_string()],
            request_body_override: None,
            thread_epoch: 1,
            thread_anchor: "thread-1".to_string(),
            reset_session_affinity: false,
            requires_replay: false,
            current_turn_index: 1,
            primary_scope_id_for_commit: Some("conv-real".to_string()),
            scope_promotion: Some(AffinityScopePromotion {
                platform_key_hash: "pk".to_string(),
                affinity_key: "sid:promote".to_string(),
                from_scope_id: "affinity::synthetic".to_string(),
                to_scope_id: "conv-real".to_string(),
            }),
            current_turn_input_items: vec![serde_json::json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "promote"}]
            })],
            selected_supply_score: Some(0.9),
            selected_pressure_score: Some(1.0),
            selected_final_score: Some(0.9),
            switch_reason: Some("promote_scope".to_string()),
        };

        super::finalize_affinity_success(
            &storage,
            &resolution,
            "pk",
            "acc-1",
            br#"{"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"promote"}]}]}"#,
            Some(
                br#"{"response":{"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]}]}}"#,
            ),
            "Passthrough",
            "openai_compat",
        )
        .expect("finalize success");

        let state = storage
            .get_conversation_context_state("pk", "sid:promote", "conv-real")
            .expect("load state")
            .expect("state exists");
        assert_eq!(state.instructions_text.as_deref(), Some("carry synthetic"));
        assert_eq!(
            state.tools_json.as_deref(),
            Some("[{\"type\":\"function\",\"name\":\"lookup\"}]")
        );
        assert_eq!(state.metadata_json.as_deref(), Some("{\"scope\":\"synthetic\"}"));

        crate::gateway::affinity::set_mode(previous_mode.as_str()).expect("restore mode");
    }

    #[test]
    fn supports_affinity_persistence_request_only_allows_passthrough_responses() {
        assert!(supports_affinity_persistence_request(
            "/v1/responses",
            "/v1/responses",
            ResponseAdapter::Passthrough,
        ));
        assert!(supports_affinity_persistence_request(
            "/v1/responses?stream=true",
            "/v1/responses?stream=true",
            ResponseAdapter::Passthrough,
        ));
        assert!(!supports_affinity_persistence_request(
            "/v1/messages",
            "/v1/responses",
            ResponseAdapter::AnthropicJson,
        ));
        assert!(!supports_affinity_persistence_request(
            "/v1/chat/completions",
            "/v1/responses",
            ResponseAdapter::OpenAIChatCompletionsJson,
        ));
        assert!(!supports_affinity_persistence_request(
            "/v1/models",
            "/v1/models",
            ResponseAdapter::Passthrough,
        ));
    }

    #[test]
    fn resolve_scope_id_defers_synthetic_scope_promotion_until_commit() {
        let storage = Storage::open_in_memory().expect("open in memory");
        storage.init().expect("init schema");
        let now = 10_i64;
        storage
            .save_client_binding(
                &ClientBinding {
                    platform_key_hash: "pk".to_string(),
                    affinity_key: "sid:test".to_string(),
                    account_id: "acc-1".to_string(),
                    primary_scope_id: Some("affinity::synthetic".to_string()),
                    binding_version: 1,
                    status: "active".to_string(),
                    last_supply_score: None,
                    last_pressure_score: None,
                    last_final_score: None,
                    last_switch_reason: Some("initial_bind".to_string()),
                    created_at: now,
                    updated_at: now,
                    last_seen_at: now,
                },
                None,
            )
            .expect("insert binding");
        storage
            .save_conversation_thread(
                &ConversationThread {
                    platform_key_hash: "pk".to_string(),
                    affinity_key: "sid:test".to_string(),
                    conversation_scope_id: "affinity::synthetic".to_string(),
                    account_id: "acc-1".to_string(),
                    thread_epoch: 1,
                    thread_anchor: "thread-synth".to_string(),
                    thread_version: 1,
                    created_at: now,
                    updated_at: now,
                    last_seen_at: now,
                },
                None,
            )
            .expect("insert thread");

        let binding = storage
            .get_client_binding("pk", "sid:test")
            .expect("get binding")
            .expect("binding exists");
        let (
            source_scope_id,
            commit_scope_id,
            primary_scope_id_for_commit,
            scope_promotion,
        ) = resolve_scope_id(&storage, "pk", "sid:test", Some(&binding), Some("conv-real"))
            .expect("resolve scope");

        assert_eq!(source_scope_id, "affinity::synthetic");
        assert_eq!(commit_scope_id, "conv-real");
        assert_eq!(primary_scope_id_for_commit.as_deref(), Some("conv-real"));
        let promotion = scope_promotion.expect("promotion draft");
        assert_eq!(promotion.from_scope_id, "affinity::synthetic");
        assert_eq!(promotion.to_scope_id, "conv-real");

        let committed_binding = storage
            .get_client_binding("pk", "sid:test")
            .expect("reload binding")
            .expect("binding still exists");
        assert_eq!(
            committed_binding.primary_scope_id.as_deref(),
            Some("affinity::synthetic")
        );
        assert!(
            storage
                .get_conversation_thread("pk", "sid:test", "conv-real")
                .expect("requested thread lookup")
                .is_none()
        );
    }

    #[test]
    fn evaluate_candidate_state_prefers_exhausted_over_cooldown() {
        crate::gateway::scheduler_set_account_cooldown_until("acc-exhausted", Some(now_ts() + 60), true);
        record_affinity_attempt_feedback("acc-exhausted", 429, Some("insufficient_quota"));
        record_affinity_attempt_feedback("acc-exhausted", 429, Some("billing_hard_limit"));

        let state = evaluate_candidate_state(
            &Account {
                id: "acc-exhausted".to_string(),
                label: "acc-exhausted".to_string(),
                issuer: "https://auth.openai.com".to_string(),
                chatgpt_account_id: None,
                workspace_id: None,
                group_name: None,
                sort: 0,
                status: "active".to_string(),
                created_at: now_ts(),
                updated_at: now_ts(),
            },
            &Token {
                account_id: "acc-exhausted".to_string(),
                id_token: "id".to_string(),
                access_token: "access".to_string(),
                refresh_token: "refresh".to_string(),
                api_key_access_token: None,
                last_refresh: now_ts(),
            },
            &crate::gateway::scheduler::SchedulerAccountSnapshot {
                remaining_quota_percent: 50.0,
                usage_known: true,
                usage_snapshot_fresh: true,
                route_health_score: 100,
                dynamic_limit: 1,
                inflight: 0,
                cooldown_until: Some(now_ts() + 60),
                latency_ewma_ms: Some(100.0),
            },
        );

        crate::gateway::clear_account_cooldown("acc-exhausted");
        assert_eq!(state, super::CandidateState::Exhausted);
    }

    #[test]
    fn finalize_affinity_success_commits_binding_thread_state_and_events_atomically() {
        let _guard = affinity_runtime_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_mode = crate::gateway::affinity::current_mode();
        crate::gateway::affinity::set_mode("enforce").expect("set enforce mode");

        let storage = Storage::open_in_memory().expect("open in memory");
        storage.init().expect("init schema");
        let resolution = super::AffinityRoutingResolution {
            affinity_key: "sid:test".to_string(),
            affinity_source: "session_id",
            conversation_scope_id: "affinity::synthetic".to_string(),
            committed_conversation_scope_id: "affinity::synthetic".to_string(),
            binding: None,
            thread: None,
            chosen_account_id: "acc-1".to_string(),
            candidate_account_ids: vec!["acc-1".to_string()],
            request_body_override: None,
            thread_epoch: 1,
            thread_anchor: "thread-1".to_string(),
            reset_session_affinity: false,
            requires_replay: false,
            current_turn_index: 0,
            primary_scope_id_for_commit: Some("affinity::synthetic".to_string()),
            scope_promotion: None,
            current_turn_input_items: vec![serde_json::json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "hello"}]
            })],
            selected_supply_score: Some(0.9),
            selected_pressure_score: Some(1.0),
            selected_final_score: Some(0.9),
            switch_reason: Some("initial_bind".to_string()),
        };

        super::finalize_affinity_success(
            &storage,
            &resolution,
            "pk",
            "acc-1",
            br#"{"model":"gpt-5.4","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}]}"#,
            Some(
                br#"{"response":{"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"world"}]}]}}"#,
            ),
            "Passthrough",
            "openai_compat",
        )
        .expect("finalize success");

        let binding = storage
            .get_client_binding("pk", "sid:test")
            .expect("load binding")
            .expect("binding exists");
        assert_eq!(binding.account_id, "acc-1");
        assert_eq!(binding.binding_version, 1);

        let thread = storage
            .get_conversation_thread("pk", "sid:test", "affinity::synthetic")
            .expect("load thread")
            .expect("thread exists");
        assert_eq!(thread.account_id, "acc-1");
        assert_eq!(thread.thread_version, 1);

        let state = storage
            .get_conversation_context_state("pk", "sid:test", "affinity::synthetic")
            .expect("load state")
            .expect("state exists");
        assert_eq!(state.model.as_deref(), Some("gpt-5.4"));

        let events = storage
            .list_conversation_context_events("pk", "sid:test", "affinity::synthetic")
            .expect("load events");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].turn_index, 0);
        assert_eq!(events[0].item_seq, 0);
        assert_eq!(events[1].item_seq, 1);

        crate::gateway::affinity::set_mode(previous_mode.as_str()).expect("restore mode");
    }

    #[test]
    fn finalize_affinity_success_persists_only_current_turn_input_items_after_replay() {
        let _guard = affinity_runtime_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_mode = crate::gateway::affinity::current_mode();
        crate::gateway::affinity::set_mode("enforce").expect("set enforce mode");

        let storage = Storage::open_in_memory().expect("open in memory");
        storage.init().expect("init schema");
        let resolution = super::AffinityRoutingResolution {
            affinity_key: "sid:test-replay".to_string(),
            affinity_source: "session_id",
            conversation_scope_id: "scope".to_string(),
            committed_conversation_scope_id: "scope".to_string(),
            binding: None,
            thread: None,
            chosen_account_id: "acc-1".to_string(),
            candidate_account_ids: vec!["acc-1".to_string()],
            request_body_override: Some(Bytes::from_static(
                br#"{"model":"gpt-5.4","input":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"history"}]},{"type":"message","role":"user","content":[{"type":"input_text","text":"next"}]}]}"#,
            )),
            thread_epoch: 1,
            thread_anchor: "thread-1".to_string(),
            reset_session_affinity: false,
            requires_replay: true,
            current_turn_index: 1,
            primary_scope_id_for_commit: Some("scope".to_string()),
            scope_promotion: None,
            current_turn_input_items: vec![serde_json::json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "next"}]
            })],
            selected_supply_score: Some(0.9),
            selected_pressure_score: Some(1.0),
            selected_final_score: Some(0.9),
            switch_reason: Some("rebind_soft_quota".to_string()),
        };

        super::finalize_affinity_success(
            &storage,
            &resolution,
            "pk",
            "acc-1",
            br#"{"model":"gpt-5.4","instructions":"carry over","input":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"history"}]},{"type":"message","role":"user","content":[{"type":"input_text","text":"next"}]}]}"#,
            Some(
                br#"{"response":{"output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]}]}}"#,
            ),
            "Passthrough",
            "openai_compat",
        )
        .expect("finalize success");

        let events = storage
            .list_conversation_context_events("pk", "sid:test-replay", "scope")
            .expect("load events");
        assert_eq!(events.len(), 2);
        assert!(events[0].item_json.contains("\"text\":\"next\""));
        assert!(!events[0].item_json.contains("\"text\":\"history\""));

        let state = storage
            .get_conversation_context_state("pk", "sid:test-replay", "scope")
            .expect("load state")
            .expect("state exists");
        assert_eq!(state.instructions_text.as_deref(), Some("carry over"));

        crate::gateway::affinity::set_mode(previous_mode.as_str()).expect("restore mode");
    }
}
