use codexmanager_core::storage::{Account, Token};
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

const ROUTE_STRATEGY_ENV: &str = "CODEXMANAGER_ROUTE_STRATEGY";
const ROUTE_RENDEZVOUS_TOP_K_ENV: &str = "CODEXMANAGER_ROUTE_RENDEZVOUS_TOP_K";
const ROUTE_ORDERED_PREFIX_WINDOW_ENV: &str = "CODEXMANAGER_ROUTE_ORDERED_PREFIX_WINDOW";

const ROUTE_MODE_ORDERED: u8 = 0;
const ROUTE_MODE_BALANCED: u8 = 1;
const ROUTE_STRATEGY_ORDERED: &str = "ordered";
const ROUTE_STRATEGY_BALANCED: &str = "balanced";

const DEFAULT_ROUTE_RENDEZVOUS_TOP_K: usize = 2;
const DEFAULT_ROUTE_ORDERED_PREFIX_WINDOW: usize = 2;
const MAX_ROUTE_WINDOW: usize = 3;

static ROUTE_MODE: AtomicU8 = AtomicU8::new(ROUTE_MODE_BALANCED);
static ROUTE_RENDEZVOUS_TOP_K: AtomicUsize = AtomicUsize::new(DEFAULT_ROUTE_RENDEZVOUS_TOP_K);
static ROUTE_ORDERED_PREFIX_WINDOW: AtomicUsize =
    AtomicUsize::new(DEFAULT_ROUTE_ORDERED_PREFIX_WINDOW);
static ROUTE_STATE: OnceLock<Mutex<RouteHintState>> = OnceLock::new();
static ROUTE_CONFIG_LOADED: OnceLock<()> = OnceLock::new();

#[derive(Default)]
struct RouteHintState {
    manual_preferred_account_id: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct RouteSelectionContext<'a> {
    key_id: &'a str,
    model: Option<&'a str>,
    flow_key: &'a str,
}

#[derive(Clone)]
struct RankedCandidate {
    account_id: String,
    rendezvous_score: u64,
    burn_score: u64,
}

impl<'a> RouteSelectionContext<'a> {
    pub(crate) fn new(key_id: &'a str, model: Option<&'a str>, flow_key: &'a str) -> Self {
        Self {
            key_id,
            model,
            flow_key,
        }
    }
}

pub(crate) fn apply_route_strategy(
    candidates: &mut Vec<(Account, Token)>,
    selection: RouteSelectionContext<'_>,
) {
    ensure_route_config_loaded();
    if candidates.len() <= 1 {
        return;
    }

    if rotate_to_manual_preferred_account(candidates) {
        return;
    }

    let candidate_set_hash = stable_candidate_set_hash(candidates.as_slice());
    hard_filter_local_unavailable_candidates(candidates);
    if candidates.len() <= 1 {
        return;
    }

    let instance_id = super::instance_id::current_instance_id();
    let mode = route_mode();
    if mode == ROUTE_MODE_BALANCED {
        apply_balanced_rendezvous(candidates, &selection, &instance_id, candidate_set_hash);
    } else {
        apply_ordered_prefix_rendezvous(candidates, &selection, &instance_id, candidate_set_hash);
    }
}

fn apply_balanced_rendezvous(
    candidates: &mut Vec<(Account, Token)>,
    selection: &RouteSelectionContext<'_>,
    instance_id: &str,
    candidate_set_hash: u64,
) {
    let ranked = ranked_candidates(
        candidates.as_slice(),
        selection,
        instance_id,
        candidate_set_hash,
    );
    let top_k = route_rendezvous_top_k().min(ranked.len());
    let selected = ranked
        .iter()
        .take(top_k)
        .min_by_key(|entry| (entry.burn_score, Reverse(entry.rendezvous_score)))
        .map(|entry| entry.account_id.clone());
    let Some(selected_id) = selected else {
        return;
    };
    reorder_candidates(candidates, ranked, selected_id.as_str());
}

fn apply_ordered_prefix_rendezvous(
    candidates: &mut Vec<(Account, Token)>,
    selection: &RouteSelectionContext<'_>,
    instance_id: &str,
    candidate_set_hash: u64,
) {
    let prefix_window = route_ordered_prefix_window().min(candidates.len());
    if prefix_window <= 1 {
        return;
    }
    let prefix_ranked = ranked_candidates(
        candidates[..prefix_window].to_vec().as_slice(),
        selection,
        instance_id,
        candidate_set_hash,
    );
    let selected = prefix_ranked
        .iter()
        .max_by_key(|entry| entry.rendezvous_score)
        .map(|entry| entry.account_id.clone());
    let Some(selected_id) = selected else {
        return;
    };
    if let Some(index) = candidates
        .iter()
        .position(|(account, _)| account.id == selected_id)
    {
        let selected = candidates.remove(index);
        candidates.insert(0, selected);
    }
}

fn ranked_candidates(
    candidates: &[(Account, Token)],
    selection: &RouteSelectionContext<'_>,
    instance_id: &str,
    candidate_set_hash: u64,
) -> Vec<RankedCandidate> {
    let mut ranked = candidates
        .iter()
        .map(|(account, _)| RankedCandidate {
            account_id: account.id.clone(),
            rendezvous_score: rendezvous_score(
                instance_id,
                selection.key_id,
                selection.model,
                selection.flow_key,
                candidate_set_hash,
                account.id.as_str(),
            ),
            burn_score: super::local_burn::local_burn_score(account.id.as_str()),
        })
        .collect::<Vec<_>>();
    ranked.sort_by_key(|entry| (Reverse(entry.rendezvous_score), entry.account_id.clone()));
    ranked
}

fn reorder_candidates(
    candidates: &mut Vec<(Account, Token)>,
    ranked: Vec<RankedCandidate>,
    selected_id: &str,
) {
    let ranked_tail = ranked
        .into_iter()
        .filter(|entry| entry.account_id != selected_id)
        .map(|entry| entry.account_id)
        .collect::<Vec<_>>();
    let mut reordered = Vec::with_capacity(candidates.len());
    if let Some(index) = candidates
        .iter()
        .position(|(account, _)| account.id == selected_id)
    {
        reordered.push(candidates.remove(index));
    }
    for account_id in ranked_tail {
        if let Some(index) = candidates
            .iter()
            .position(|(account, _)| account.id == account_id)
        {
            reordered.push(candidates.remove(index));
        }
    }
    reordered.append(candidates);
    *candidates = reordered;
}

fn hard_filter_local_unavailable_candidates(candidates: &mut Vec<(Account, Token)>) {
    let inflight_limit = super::runtime_config::account_max_inflight_limit();
    let mut available = Vec::with_capacity(candidates.len());
    let mut inflight_only = Vec::new();

    for candidate in candidates.drain(..) {
        let account_id = candidate.0.id.as_str();
        if super::cooldown::is_account_in_cooldown(account_id) {
            continue;
        }
        let inflight_saturated = inflight_limit > 0
            && super::metrics::account_inflight_count(account_id) >= inflight_limit;
        if inflight_saturated {
            inflight_only.push(candidate);
        } else {
            available.push(candidate);
        }
    }

    if !available.is_empty() {
        *candidates = available;
    } else {
        // 中文注释：当账号只是“全部都忙”而不是“全部都不可用”时，不要提前把候选池清空；
        // 保留这些 inflight 候选，让后续 per-account gate 负责等待/切换，避免直接 503 no_available_account。
        *candidates = inflight_only;
    }
}

fn rotate_to_manual_preferred_account(candidates: &mut Vec<(Account, Token)>) -> bool {
    let lock = ROUTE_STATE.get_or_init(|| Mutex::new(RouteHintState::default()));
    let state = crate::lock_utils::lock_recover(lock, "route_hint_state");
    let Some(account_id) = state.manual_preferred_account_id.as_deref() else {
        return false;
    };
    let Some(index) = candidates
        .iter()
        .position(|(account, _)| account.id.eq(account_id))
    else {
        return false;
    };
    if index > 0 {
        candidates.rotate_left(index);
    }
    true
}

fn route_mode() -> u8 {
    ROUTE_MODE.load(Ordering::Relaxed)
}

fn route_mode_label(mode: u8) -> &'static str {
    if mode == ROUTE_MODE_BALANCED {
        ROUTE_STRATEGY_BALANCED
    } else {
        ROUTE_STRATEGY_ORDERED
    }
}

fn parse_route_mode(raw: &str) -> Option<u8> {
    match raw.trim().to_ascii_lowercase().as_str() {
        ROUTE_STRATEGY_ORDERED | "order" | "priority" | "sequential" => Some(ROUTE_MODE_ORDERED),
        ROUTE_STRATEGY_BALANCED | "round_robin" | "round-robin" | "rr" => Some(ROUTE_MODE_BALANCED),
        _ => None,
    }
}

pub(crate) fn current_route_strategy() -> &'static str {
    ensure_route_config_loaded();
    route_mode_label(route_mode())
}

pub(crate) fn set_route_strategy(strategy: &str) -> Result<&'static str, String> {
    let Some(mode) = parse_route_mode(strategy) else {
        return Err(
            "invalid strategy; use ordered or balanced (aliases: round_robin/round-robin/rr)"
                .to_string(),
        );
    };
    ROUTE_MODE.store(mode, Ordering::Relaxed);
    Ok(route_mode_label(mode))
}

pub(crate) fn get_manual_preferred_account() -> Option<String> {
    ensure_route_config_loaded();
    let lock = ROUTE_STATE.get_or_init(|| Mutex::new(RouteHintState::default()));
    let state = crate::lock_utils::lock_recover(lock, "route_hint_state");
    state.manual_preferred_account_id.clone()
}

pub(crate) fn set_manual_preferred_account(account_id: &str) -> Result<(), String> {
    ensure_route_config_loaded();
    let id = account_id.trim();
    if id.is_empty() {
        return Err("accountId is required".to_string());
    }
    let lock = ROUTE_STATE.get_or_init(|| Mutex::new(RouteHintState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "route_hint_state");
    state.manual_preferred_account_id = Some(id.to_string());
    Ok(())
}

pub(crate) fn clear_manual_preferred_account() {
    ensure_route_config_loaded();
    let lock = ROUTE_STATE.get_or_init(|| Mutex::new(RouteHintState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "route_hint_state");
    state.manual_preferred_account_id = None;
}

pub(crate) fn clear_manual_preferred_account_if(account_id: &str) -> bool {
    ensure_route_config_loaded();
    let id = account_id.trim();
    if id.is_empty() {
        return false;
    }
    let lock = ROUTE_STATE.get_or_init(|| Mutex::new(RouteHintState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "route_hint_state");
    if state
        .manual_preferred_account_id
        .as_deref()
        .is_some_and(|current| current == id)
    {
        state.manual_preferred_account_id = None;
        return true;
    }
    false
}

pub(super) fn reload_from_env() {
    let raw = std::env::var(ROUTE_STRATEGY_ENV).unwrap_or_default();
    let mode = parse_route_mode(raw.as_str()).unwrap_or(ROUTE_MODE_BALANCED);
    ROUTE_MODE.store(mode, Ordering::Relaxed);
    ROUTE_RENDEZVOUS_TOP_K.store(
        env_usize_or(ROUTE_RENDEZVOUS_TOP_K_ENV, DEFAULT_ROUTE_RENDEZVOUS_TOP_K)
            .clamp(1, MAX_ROUTE_WINDOW),
        Ordering::Relaxed,
    );
    ROUTE_ORDERED_PREFIX_WINDOW.store(
        env_usize_or(
            ROUTE_ORDERED_PREFIX_WINDOW_ENV,
            DEFAULT_ROUTE_ORDERED_PREFIX_WINDOW,
        )
        .clamp(1, MAX_ROUTE_WINDOW),
        Ordering::Relaxed,
    );
    if let Some(lock) = ROUTE_STATE.get() {
        let mut state = crate::lock_utils::lock_recover(lock, "route_hint_state");
        state.manual_preferred_account_id = None;
    }
}

fn ensure_route_config_loaded() {
    let _ = ROUTE_CONFIG_LOADED.get_or_init(|| reload_from_env());
}

fn route_rendezvous_top_k() -> usize {
    ROUTE_RENDEZVOUS_TOP_K
        .load(Ordering::Relaxed)
        .clamp(1, MAX_ROUTE_WINDOW)
}

fn route_ordered_prefix_window() -> usize {
    ROUTE_ORDERED_PREFIX_WINDOW
        .load(Ordering::Relaxed)
        .clamp(1, MAX_ROUTE_WINDOW)
}

fn env_usize_or(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn stable_candidate_set_hash(candidates: &[(Account, Token)]) -> u64 {
    let mut account_ids = candidates
        .iter()
        .map(|(account, _)| account.id.as_str())
        .collect::<Vec<_>>();
    account_ids.sort_unstable();
    stable_hash_u64(account_ids.join("|").as_bytes())
}

fn rendezvous_score(
    instance_id: &str,
    key_id: &str,
    model: Option<&str>,
    flow_key: &str,
    candidate_set_hash: u64,
    account_id: &str,
) -> u64 {
    stable_hash_u64(
        format!(
            "{}|{}|{}|{}|{}|{}",
            instance_id,
            key_id.trim(),
            model
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("-"),
            flow_key.trim(),
            candidate_set_hash,
            account_id.trim(),
        )
        .as_bytes(),
    )
}

fn stable_hash_u64(input: &[u8]) -> u64 {
    let digest = Sha256::digest(input);
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(bytes)
}

#[cfg(test)]
pub(crate) fn clear_route_state_for_tests() {
    super::local_burn::clear_runtime_state_for_tests();
    super::instance_id::clear_instance_id_for_tests();
    if let Some(lock) = ROUTE_STATE.get() {
        let mut state = crate::lock_utils::lock_recover(lock, "route_hint_state");
        state.manual_preferred_account_id = None;
    }
}

#[cfg(test)]
fn route_strategy_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static ROUTE_STRATEGY_TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    crate::lock_utils::lock_recover(
        ROUTE_STRATEGY_TEST_MUTEX.get_or_init(|| Mutex::new(())),
        "route strategy test mutex",
    )
}

#[cfg(test)]
#[path = "tests/route_hint_tests.rs"]
mod tests;
