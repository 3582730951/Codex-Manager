use codexmanager_core::storage::{now_ts, Account, Storage, Token, UsageSnapshotRecord};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::account_plan::resolve_account_plan;

const DEFAULT_ROUTE_HEALTH_SCORE: i32 = 100;
const DEFAULT_DYNAMIC_LIMIT: usize = 1;
const DEFAULT_USAGE_CACHE_TTL_MS: u64 = 5_000;

#[derive(Debug, Clone)]
struct SchedulerAccountRuntime {
    last_assigned_at: i64,
    latency_ewma_ms: Option<f64>,
    success_ewma: Option<f64>,
    network_penalty_ewma: Option<f64>,
    rate_limit_penalty_ewma: Option<f64>,
    stream_penalty_ewma: Option<f64>,
    inflight: usize,
    cooldown_until: Option<i64>,
    route_health_score: i32,
    dynamic_limit: usize,
}

impl Default for SchedulerAccountRuntime {
    fn default() -> Self {
        Self {
            last_assigned_at: 0,
            latency_ewma_ms: None,
            success_ewma: None,
            network_penalty_ewma: None,
            rate_limit_penalty_ewma: None,
            stream_penalty_ewma: None,
            inflight: 0,
            cooldown_until: None,
            route_health_score: DEFAULT_ROUTE_HEALTH_SCORE,
            dynamic_limit: DEFAULT_DYNAMIC_LIMIT,
        }
    }
}

#[derive(Default)]
struct UsageSnapshotCache {
    by_account: HashMap<String, UsageSnapshotRecord>,
    refreshed_at: Option<Instant>,
}

#[derive(Default)]
struct SchedulerState {
    accounts: HashMap<String, SchedulerAccountRuntime>,
    usage_cache: UsageSnapshotCache,
    waiting_requests: usize,
}

struct SchedulerRuntime {
    state: Mutex<SchedulerState>,
    changed: Condvar,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SchedulerFeedback {
    pub(crate) status_code: u16,
    pub(crate) elapsed_ms: u64,
    pub(crate) network_error: bool,
    pub(crate) stream_failed: bool,
}

static SCHEDULER_RUNTIME: OnceLock<SchedulerRuntime> = OnceLock::new();

fn runtime() -> &'static SchedulerRuntime {
    SCHEDULER_RUNTIME.get_or_init(|| SchedulerRuntime {
        state: Mutex::new(SchedulerState::default()),
        changed: Condvar::new(),
    })
}

fn ewma(prev: Option<f64>, next: f64) -> f64 {
    match prev {
        Some(prev) => prev * 0.75 + next * 0.25,
        None => next,
    }
}

fn usage_cache_ttl() -> Duration {
    Duration::from_millis(DEFAULT_USAGE_CACHE_TTL_MS)
}

fn is_usage_cache_stale(cache: &UsageSnapshotCache) -> bool {
    match cache.refreshed_at {
        Some(refreshed_at) => refreshed_at.elapsed() >= usage_cache_ttl(),
        None => true,
    }
}

fn load_usage_cache_from_storage(storage: &Storage) -> HashMap<String, UsageSnapshotRecord> {
    storage
        .latest_usage_snapshots_by_account()
        .unwrap_or_default()
        .into_iter()
        .map(|snapshot| (snapshot.account_id.clone(), snapshot))
        .collect()
}

fn ensure_usage_cache(storage: &Storage) {
    let needs_refresh = {
        let state = crate::lock_utils::lock_recover(&runtime().state, "scheduler_state");
        is_usage_cache_stale(&state.usage_cache)
    };
    if !needs_refresh {
        return;
    }

    let refreshed = load_usage_cache_from_storage(storage);
    let mut state = crate::lock_utils::lock_recover(&runtime().state, "scheduler_state");
    if is_usage_cache_stale(&state.usage_cache) {
        state.usage_cache.by_account = refreshed;
        state.usage_cache.refreshed_at = Some(Instant::now());
    }
}

fn remaining_quota_percent(snapshot: Option<&UsageSnapshotRecord>) -> f64 {
    let Some(snapshot) = snapshot else {
        return 50.0;
    };
    let primary = snapshot
        .used_percent
        .map(|value| (100.0 - value).clamp(0.0, 100.0));
    let secondary = snapshot
        .secondary_used_percent
        .map(|value| (100.0 - value).clamp(0.0, 100.0));
    match (primary, secondary) {
        (Some(primary), Some(secondary)) => primary.min(secondary),
        (Some(primary), None) => primary,
        (None, Some(secondary)) => secondary,
        (None, None) => 50.0,
    }
}

fn base_capacity_for_candidate(token: &Token, snapshot: Option<&UsageSnapshotRecord>) -> usize {
    let resolved = resolve_account_plan(Some(token), snapshot);
    match resolved
        .as_ref()
        .map(|plan| plan.normalized.as_str())
        .unwrap_or("unknown")
    {
        "enterprise" | "business" => 4,
        "team" | "pro" => 3,
        "plus" | "go" => 2,
        "free" | "edu" => 1,
        _ => 2,
    }
}

fn cap_by_runtime_signals(
    mut capacity: usize,
    remaining_quota: f64,
    route_health: i32,
    runtime: &SchedulerAccountRuntime,
) -> usize {
    if remaining_quota <= 10.0 {
        capacity = 1;
    } else if remaining_quota <= 25.0 {
        capacity = capacity.min(2);
    }

    if route_health < 60 {
        capacity = 1;
    } else if route_health < 90 {
        capacity = capacity.min(2);
    }

    if runtime
        .latency_ewma_ms
        .is_some_and(|latency_ms| latency_ms > 15_000.0)
    {
        capacity = 1;
    } else if runtime
        .latency_ewma_ms
        .is_some_and(|latency_ms| latency_ms > 6_000.0)
    {
        capacity = capacity.min(2);
    }

    if runtime
        .network_penalty_ewma
        .is_some_and(|value| value >= 0.45)
        || runtime
            .stream_penalty_ewma
            .is_some_and(|value| value >= 0.35)
        || runtime
            .rate_limit_penalty_ewma
            .is_some_and(|value| value >= 0.35)
    {
        capacity = 1;
    }

    capacity.max(1)
}

fn dynamic_limit_for_candidate(
    token: &Token,
    snapshot: Option<&UsageSnapshotRecord>,
    route_health: i32,
    runtime: &SchedulerAccountRuntime,
    static_limit: usize,
) -> usize {
    let base = base_capacity_for_candidate(token, snapshot);
    let remaining_quota = remaining_quota_percent(snapshot);
    let dynamic = cap_by_runtime_signals(base, remaining_quota, route_health, runtime);
    if static_limit == 0 {
        dynamic
    } else {
        dynamic.min(static_limit.max(1))
    }
}

fn fairness_score(runtime: &SchedulerAccountRuntime) -> f64 {
    let age_secs = now_ts().saturating_sub(runtime.last_assigned_at);
    if age_secs <= 0 {
        return 1.0;
    }
    (age_secs as f64 / 10.0).clamp(0.0, 1.0)
}

fn success_score(route_health: i32, runtime: &SchedulerAccountRuntime) -> f64 {
    let health = f64::from(route_health.clamp(0, 200)) / 200.0;
    let runtime_success = runtime.success_ewma.unwrap_or(health);
    ((health * 0.5) + (runtime_success * 0.5)).clamp(0.0, 1.0)
}

fn latency_score(runtime: &SchedulerAccountRuntime) -> f64 {
    let Some(latency_ms) = runtime.latency_ewma_ms else {
        return 0.7;
    };
    (1.0 / (1.0 + latency_ms / 2_500.0)).clamp(0.0, 1.0)
}

fn account_score(
    remaining_quota: f64,
    route_health: i32,
    limit: usize,
    runtime: &SchedulerAccountRuntime,
) -> f64 {
    let quota_score = (remaining_quota / 100.0).clamp(0.0, 1.0);
    let health_score = success_score(route_health, runtime);
    let latency_score = latency_score(runtime);
    let headroom_score = if limit == 0 {
        1.0
    } else {
        (limit.saturating_sub(runtime.inflight) as f64 / limit as f64).clamp(0.0, 1.0)
    };
    let fairness_score = fairness_score(runtime);
    quota_score * 0.40
        + health_score * 0.25
        + latency_score * 0.15
        + headroom_score * 0.10
        + fairness_score * 0.10
}

fn account_cooldown_remaining(
    runtime: Option<&SchedulerAccountRuntime>,
    now: i64,
) -> Option<Duration> {
    let until = runtime.and_then(|runtime| runtime.cooldown_until)?;
    if until <= now {
        return None;
    }
    Some(Duration::from_secs((until - now) as u64))
}

fn wait_limit_for_account(
    runtime: Option<&SchedulerAccountRuntime>,
    fallback_limit: usize,
) -> usize {
    runtime
        .map(|runtime| runtime.dynamic_limit.max(1))
        .unwrap_or_else(|| fallback_limit.max(1))
}

pub(crate) fn rebalance_candidates(
    storage: &Storage,
    candidates: &mut Vec<(Account, Token)>,
    static_limit: usize,
    preserve_head: bool,
) -> HashMap<String, usize> {
    ensure_usage_cache(storage);
    let mut state = crate::lock_utils::lock_recover(&runtime().state, "scheduler_state");
    let sort_start = if preserve_head && candidates.len() > 1 {
        1
    } else {
        0
    };
    let mut limits = HashMap::with_capacity(candidates.len());
    let mut scored = Vec::with_capacity(candidates.len().saturating_sub(sort_start));

    for (original_idx, (account, token)) in candidates.iter().enumerate().skip(sort_start) {
        let snapshot = state
            .usage_cache
            .by_account
            .get(account.id.as_str())
            .cloned();
        let remaining_quota = remaining_quota_percent(snapshot.as_ref());
        let runtime = state.accounts.entry(account.id.clone()).or_default();
        let route_health = runtime.route_health_score;
        let limit = dynamic_limit_for_candidate(
            token,
            snapshot.as_ref(),
            route_health,
            runtime,
            static_limit,
        );
        runtime.dynamic_limit = limit;
        let score = account_score(remaining_quota, route_health, limit, runtime);
        limits.insert(account.id.clone(), limit);
        scored.push((original_idx, score));
    }

    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    if sort_start < candidates.len() {
        let original = candidates[sort_start..].to_vec();
        let reordered = scored
            .iter()
            .filter_map(|(original_idx, _)| original.get(*original_idx - sort_start).cloned())
            .collect::<Vec<_>>();
        candidates.splice(sort_start.., reordered);
    }

    limits
}

pub(crate) fn record_assignment(account_id: &str) {
    let mut state = crate::lock_utils::lock_recover(&runtime().state, "scheduler_state");
    let entry = state.accounts.entry(account_id.to_string()).or_default();
    entry.last_assigned_at = now_ts();
}

pub(crate) fn record_feedback(account_id: &str, feedback: SchedulerFeedback) {
    let mut state = crate::lock_utils::lock_recover(&runtime().state, "scheduler_state");
    let entry = state.accounts.entry(account_id.to_string()).or_default();
    entry.latency_ewma_ms = Some(ewma(
        entry.latency_ewma_ms,
        feedback.elapsed_ms.max(1) as f64,
    ));
    entry.success_ewma = Some(ewma(
        entry.success_ewma,
        if (200..=299).contains(&feedback.status_code) {
            1.0
        } else {
            0.0
        },
    ));
    entry.network_penalty_ewma = Some(ewma(
        entry.network_penalty_ewma,
        if feedback.network_error { 1.0 } else { 0.0 },
    ));
    entry.rate_limit_penalty_ewma = Some(ewma(
        entry.rate_limit_penalty_ewma,
        if feedback.status_code == 429 {
            1.0
        } else {
            0.0
        },
    ));
    entry.stream_penalty_ewma = Some(ewma(
        entry.stream_penalty_ewma,
        if feedback.stream_failed { 1.0 } else { 0.0 },
    ));
}

pub(crate) fn record_route_health(account_id: &str, route_health: i32) {
    let mut state = crate::lock_utils::lock_recover(&runtime().state, "scheduler_state");
    let entry = state.accounts.entry(account_id.to_string()).or_default();
    entry.route_health_score = route_health.clamp(0, 200);
}

pub(crate) fn store_usage_snapshot(record: &UsageSnapshotRecord) {
    let mut state = crate::lock_utils::lock_recover(&runtime().state, "scheduler_state");
    state
        .usage_cache
        .by_account
        .insert(record.account_id.clone(), record.clone());
    state.usage_cache.refreshed_at = Some(Instant::now());
}

pub(crate) fn set_account_inflight(account_id: &str, inflight: usize, wake_waiters: bool) {
    let mut state = crate::lock_utils::lock_recover(&runtime().state, "scheduler_state");
    let entry = state.accounts.entry(account_id.to_string()).or_default();
    entry.inflight = inflight;
    if wake_waiters {
        runtime().changed.notify_all();
    }
}

pub(crate) fn set_account_cooldown_until(
    account_id: &str,
    cooldown_until: Option<i64>,
    wake_waiters: bool,
) {
    let mut state = crate::lock_utils::lock_recover(&runtime().state, "scheduler_state");
    let entry = state.accounts.entry(account_id.to_string()).or_default();
    entry.cooldown_until = cooldown_until;
    if wake_waiters {
        runtime().changed.notify_all();
    }
}

pub(crate) fn cached_account_inflight(account_id: &str) -> usize {
    let state = crate::lock_utils::lock_recover(&runtime().state, "scheduler_state");
    state
        .accounts
        .get(account_id)
        .map(|runtime| runtime.inflight)
        .unwrap_or(0)
}

pub(crate) fn cached_account_in_cooldown(account_id: &str) -> bool {
    let now = now_ts();
    let state = crate::lock_utils::lock_recover(&runtime().state, "scheduler_state");
    account_cooldown_remaining(state.accounts.get(account_id), now).is_some()
}

pub(crate) fn wait_for_candidate_window(
    candidates: &[(Account, Token)],
    account_dynamic_limits: &HashMap<String, usize>,
    request_deadline: Option<Instant>,
) -> bool {
    let runtime = runtime();
    let mut state = crate::lock_utils::lock_recover(&runtime.state, "scheduler_state");
    state.waiting_requests = state.waiting_requests.saturating_add(1);

    loop {
        let now = now_ts();
        let mut next_cooldown_wait: Option<Duration> = None;

        let ready = candidates.iter().any(|(account, _)| {
            let fallback_limit = account_dynamic_limits
                .get(account.id.as_str())
                .copied()
                .unwrap_or(DEFAULT_DYNAMIC_LIMIT);
            let runtime = state.accounts.get(account.id.as_str());
            let limit = wait_limit_for_account(runtime, fallback_limit);
            let inflight = runtime.map(|runtime| runtime.inflight).unwrap_or(0);
            let cooldown_remaining = account_cooldown_remaining(runtime, now);
            if let Some(remaining) = cooldown_remaining {
                next_cooldown_wait = Some(match next_cooldown_wait {
                    Some(current) => current.min(remaining),
                    None => remaining,
                });
                return false;
            }
            inflight < limit
        });

        if ready {
            state.waiting_requests = state.waiting_requests.saturating_sub(1);
            return true;
        }

        let remaining_deadline = remaining_deadline(request_deadline);
        if remaining_deadline.is_some_and(|remaining| remaining.is_zero()) {
            state.waiting_requests = state.waiting_requests.saturating_sub(1);
            return false;
        }

        let wait_for = match (remaining_deadline, next_cooldown_wait) {
            (Some(deadline), Some(cooldown)) => Some(deadline.min(cooldown)),
            (Some(deadline), None) => Some(deadline),
            (None, Some(cooldown)) => Some(cooldown),
            (None, None) => None,
        };

        match wait_for {
            Some(wait_for) if wait_for.is_zero() => continue,
            Some(wait_for) => match runtime.changed.wait_timeout(state, wait_for) {
                Ok((next_state, _)) => state = next_state,
                Err(poisoned) => {
                    let (mut next_state, _) = poisoned.into_inner();
                    next_state.waiting_requests = next_state.waiting_requests.saturating_sub(1);
                    return false;
                }
            },
            None => match runtime.changed.wait(state) {
                Ok(next_state) => state = next_state,
                Err(poisoned) => {
                    let mut next_state = poisoned.into_inner();
                    next_state.waiting_requests = next_state.waiting_requests.saturating_sub(1);
                    return false;
                }
            },
        }
    }
}

fn remaining_deadline(deadline: Option<Instant>) -> Option<Duration> {
    deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()))
}

pub(super) fn clear_runtime_state() {
    let mut state = crate::lock_utils::lock_recover(&runtime().state, "scheduler_state");
    state.accounts.clear();
    state.usage_cache.by_account.clear();
    state.usage_cache.refreshed_at = None;
    state.waiting_requests = 0;
    runtime().changed.notify_all();
}

#[cfg(test)]
fn clear_scheduler_for_tests() {
    clear_runtime_state();
}

#[cfg(test)]
mod tests {
    use super::{clear_scheduler_for_tests, rebalance_candidates, wait_for_candidate_window};
    use codexmanager_core::storage::{now_ts, Account, Storage, Token, UsageSnapshotRecord};
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    fn account(id: &str, sort: i64) -> Account {
        Account {
            id: id.to_string(),
            label: id.to_string(),
            issuer: "issuer".to_string(),
            chatgpt_account_id: None,
            workspace_id: None,
            group_name: None,
            sort,
            status: "active".to_string(),
            created_at: now_ts(),
            updated_at: now_ts(),
        }
    }

    fn token(account_id: &str) -> Token {
        Token {
            account_id: account_id.to_string(),
            id_token: String::new(),
            access_token: "token".to_string(),
            refresh_token: "refresh".to_string(),
            api_key_access_token: None,
            last_refresh: now_ts(),
        }
    }

    #[test]
    fn rebalance_candidates_prefers_healthier_and_higher_quota_accounts() {
        clear_scheduler_for_tests();
        crate::gateway::clear_account_cooldown("acc-a");
        crate::gateway::clear_account_cooldown("acc-b");
        let storage = Storage::open_in_memory().expect("open");
        storage.init().expect("init");
        storage
            .insert_usage_snapshot(&UsageSnapshotRecord {
                account_id: "acc-a".to_string(),
                used_percent: Some(95.0),
                window_minutes: Some(300),
                resets_at: None,
                secondary_used_percent: None,
                secondary_window_minutes: None,
                secondary_resets_at: None,
                credits_json: Some(r#"{"planType":"free"}"#.to_string()),
                captured_at: now_ts(),
            })
            .expect("insert acc-a usage");
        storage
            .insert_usage_snapshot(&UsageSnapshotRecord {
                account_id: "acc-b".to_string(),
                used_percent: Some(10.0),
                window_minutes: Some(300),
                resets_at: None,
                secondary_used_percent: Some(15.0),
                secondary_window_minutes: Some(10080),
                secondary_resets_at: None,
                credits_json: Some(r#"{"planType":"team"}"#.to_string()),
                captured_at: now_ts(),
            })
            .expect("insert acc-b usage");
        for _ in 0..4 {
            crate::gateway::record_route_quality("acc-a", 429);
            crate::gateway::record_route_quality("acc-b", 200);
        }
        let mut candidates = vec![
            (account("acc-a", 0), token("acc-a")),
            (account("acc-b", 1), token("acc-b")),
        ];

        rebalance_candidates(&storage, &mut candidates, 0, false);

        assert_eq!(candidates[0].0.id, "acc-b");
    }

    #[test]
    fn wait_for_candidate_window_returns_immediately_when_candidate_ready() {
        clear_scheduler_for_tests();
        let candidates = vec![(account("acc-a", 0), token("acc-a"))];
        let limits = HashMap::from([(String::from("acc-a"), 1usize)]);

        assert!(wait_for_candidate_window(
            candidates.as_slice(),
            &limits,
            Some(Instant::now() + Duration::from_millis(20))
        ));
    }
}
