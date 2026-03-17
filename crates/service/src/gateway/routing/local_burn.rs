use super::request_log::RequestLogUsage;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const DEFAULT_ROUTE_BURN_TTL_SECS: u64 = 30 * 60;
const DEFAULT_ROUTE_BURN_CAPACITY: usize = 8192;
const DEFAULT_ROUTE_BURN_ESTIMATE_TOKENS: u64 = 512;
const DEFAULT_ROUTE_STREAM_INCOMPLETE_THRESHOLD: usize = 2;
const DEFAULT_ROUTE_STREAM_INCOMPLETE_WINDOW_SECS: u64 = 120;
const ROUTE_BURN_MAINTENANCE_EVERY: u64 = 64;

const ENV_ROUTE_BURN_TTL_SECS: &str = "CODEXMANAGER_ROUTE_BURN_TTL_SECS";
const ENV_ROUTE_BURN_CAPACITY: &str = "CODEXMANAGER_ROUTE_BURN_CAPACITY";
const ENV_ROUTE_BURN_ESTIMATE_TOKENS: &str = "CODEXMANAGER_ROUTE_BURN_ESTIMATE_TOKENS";
const ENV_ROUTE_STREAM_INCOMPLETE_THRESHOLD: &str =
    "CODEXMANAGER_ROUTE_STREAM_INCOMPLETE_THRESHOLD";
const ENV_ROUTE_STREAM_INCOMPLETE_WINDOW_SECS: &str =
    "CODEXMANAGER_ROUTE_STREAM_INCOMPLETE_WINDOW_SECS";

static ROUTE_BURN_TTL_SECS: AtomicU64 = AtomicU64::new(DEFAULT_ROUTE_BURN_TTL_SECS);
static ROUTE_BURN_CAPACITY: AtomicUsize = AtomicUsize::new(DEFAULT_ROUTE_BURN_CAPACITY);
static ROUTE_BURN_ESTIMATE_TOKENS: AtomicU64 = AtomicU64::new(DEFAULT_ROUTE_BURN_ESTIMATE_TOKENS);
static ROUTE_STREAM_INCOMPLETE_THRESHOLD: AtomicUsize =
    AtomicUsize::new(DEFAULT_ROUTE_STREAM_INCOMPLETE_THRESHOLD);
static ROUTE_STREAM_INCOMPLETE_WINDOW_SECS: AtomicU64 =
    AtomicU64::new(DEFAULT_ROUTE_STREAM_INCOMPLETE_WINDOW_SECS);
static ROUTE_BURN_CONFIG_LOADED: OnceLock<()> = OnceLock::new();
static LOCAL_BURN_STATE: OnceLock<Mutex<LocalBurnState>> = OnceLock::new();

#[derive(Clone, Copy)]
struct BurnEntry {
    total_tokens: u64,
    last_seen: Instant,
}

#[derive(Clone, Copy)]
struct StrikeEntry {
    strikes: usize,
    last_seen: Instant,
}

#[derive(Default)]
struct LocalBurnState {
    burn_by_account: HashMap<String, BurnEntry>,
    incomplete_strikes_by_account: HashMap<String, StrikeEntry>,
    maintenance_tick: u64,
}

pub(crate) fn local_burn_score(account_id: &str) -> u64 {
    ensure_local_burn_config_loaded();
    let lock = LOCAL_BURN_STATE.get_or_init(|| Mutex::new(LocalBurnState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "local_burn_state");
    let now = Instant::now();
    state.maybe_maintain(now);
    state
        .burn_by_account
        .get(account_id)
        .map(|entry| entry.total_tokens)
        .unwrap_or(0)
}

pub(crate) fn record_request_usage(account_id: &str, usage: RequestLogUsage) -> u64 {
    ensure_local_burn_config_loaded();
    let tokens = usage_tokens_or_fallback(usage);
    let lock = LOCAL_BURN_STATE.get_or_init(|| Mutex::new(LocalBurnState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "local_burn_state");
    let now = Instant::now();
    state.maybe_maintain(now);
    let entry = state
        .burn_by_account
        .entry(account_id.to_string())
        .or_insert(BurnEntry {
            total_tokens: 0,
            last_seen: now,
        });
    entry.total_tokens = entry.total_tokens.saturating_add(tokens);
    entry.last_seen = now;
    state.incomplete_strikes_by_account.remove(account_id);
    enforce_burn_capacity(&mut state.burn_by_account, route_burn_capacity());
    tokens
}

pub(crate) fn record_stream_incomplete_unknown(account_id: &str) -> bool {
    ensure_local_burn_config_loaded();
    let lock = LOCAL_BURN_STATE.get_or_init(|| Mutex::new(LocalBurnState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "local_burn_state");
    let now = Instant::now();
    state.maybe_maintain(now);
    let window = route_stream_incomplete_window();
    let threshold = route_stream_incomplete_threshold();
    let entry = state
        .incomplete_strikes_by_account
        .entry(account_id.to_string())
        .or_insert(StrikeEntry {
            strikes: 0,
            last_seen: now,
        });
    let within_window = now
        .checked_duration_since(entry.last_seen)
        .is_some_and(|age| age <= window);
    if within_window {
        entry.strikes = entry.strikes.saturating_add(1);
    } else {
        entry.strikes = 1;
    }
    entry.last_seen = now;
    entry.strikes >= threshold
}

pub(crate) fn reload_from_env() {
    ROUTE_BURN_TTL_SECS.store(
        env_u64_or(ENV_ROUTE_BURN_TTL_SECS, DEFAULT_ROUTE_BURN_TTL_SECS),
        Ordering::Relaxed,
    );
    ROUTE_BURN_CAPACITY.store(
        env_usize_or(ENV_ROUTE_BURN_CAPACITY, DEFAULT_ROUTE_BURN_CAPACITY),
        Ordering::Relaxed,
    );
    ROUTE_BURN_ESTIMATE_TOKENS.store(
        env_u64_or(
            ENV_ROUTE_BURN_ESTIMATE_TOKENS,
            DEFAULT_ROUTE_BURN_ESTIMATE_TOKENS,
        ),
        Ordering::Relaxed,
    );
    ROUTE_STREAM_INCOMPLETE_THRESHOLD.store(
        env_usize_or(
            ENV_ROUTE_STREAM_INCOMPLETE_THRESHOLD,
            DEFAULT_ROUTE_STREAM_INCOMPLETE_THRESHOLD,
        )
        .max(1),
        Ordering::Relaxed,
    );
    ROUTE_STREAM_INCOMPLETE_WINDOW_SECS.store(
        env_u64_or(
            ENV_ROUTE_STREAM_INCOMPLETE_WINDOW_SECS,
            DEFAULT_ROUTE_STREAM_INCOMPLETE_WINDOW_SECS,
        ),
        Ordering::Relaxed,
    );
}

fn usage_tokens_or_fallback(usage: RequestLogUsage) -> u64 {
    usage
        .total_tokens
        .filter(|value| *value > 0)
        .map(|value| value as u64)
        .or_else(|| {
            let input = usage.input_tokens.unwrap_or(0).max(0) as u64;
            let output = usage.output_tokens.unwrap_or(0).max(0) as u64;
            let combined = input.saturating_add(output);
            (combined > 0).then_some(combined)
        })
        .unwrap_or_else(route_burn_estimate_tokens)
}

fn ensure_local_burn_config_loaded() {
    let _ = ROUTE_BURN_CONFIG_LOADED.get_or_init(|| reload_from_env());
}

fn route_burn_ttl() -> Duration {
    Duration::from_secs(ROUTE_BURN_TTL_SECS.load(Ordering::Relaxed))
}

fn route_burn_capacity() -> usize {
    ROUTE_BURN_CAPACITY.load(Ordering::Relaxed)
}

fn route_burn_estimate_tokens() -> u64 {
    ROUTE_BURN_ESTIMATE_TOKENS.load(Ordering::Relaxed).max(1)
}

fn route_stream_incomplete_threshold() -> usize {
    ROUTE_STREAM_INCOMPLETE_THRESHOLD
        .load(Ordering::Relaxed)
        .max(1)
}

fn route_stream_incomplete_window() -> Duration {
    Duration::from_secs(ROUTE_STREAM_INCOMPLETE_WINDOW_SECS.load(Ordering::Relaxed))
}

fn env_u64_or(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_usize_or(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn entry_expired(last_seen: Instant, now: Instant, ttl: Duration) -> bool {
    ttl > Duration::ZERO
        && now
            .checked_duration_since(last_seen)
            .is_some_and(|age| age > ttl)
}

fn enforce_burn_capacity(map: &mut HashMap<String, BurnEntry>, capacity: usize) {
    if capacity == 0 || map.len() <= capacity {
        return;
    }
    while map.len() > capacity {
        let Some(key) = map
            .iter()
            .min_by_key(|(key, entry)| (entry.last_seen, *key))
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        map.remove(key.as_str());
    }
}

impl LocalBurnState {
    fn maybe_maintain(&mut self, now: Instant) {
        self.maintenance_tick = self.maintenance_tick.wrapping_add(1);
        if self.maintenance_tick % ROUTE_BURN_MAINTENANCE_EVERY != 0 {
            return;
        }
        let ttl = route_burn_ttl();
        let strike_window = route_stream_incomplete_window();
        self.burn_by_account
            .retain(|_, entry| !entry_expired(entry.last_seen, now, ttl));
        self.incomplete_strikes_by_account
            .retain(|_, entry| !entry_expired(entry.last_seen, now, strike_window));
        enforce_burn_capacity(&mut self.burn_by_account, route_burn_capacity());
    }
}

#[cfg(test)]
pub(crate) fn clear_runtime_state_for_tests() {
    reload_from_env();
    let lock = LOCAL_BURN_STATE.get_or_init(|| Mutex::new(LocalBurnState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "local_burn_state");
    state.burn_by_account.clear();
    state.incomplete_strikes_by_account.clear();
    state.maintenance_tick = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.original {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn record_request_usage_prefers_total_then_combined_then_fallback() {
        clear_runtime_state_for_tests();

        let recorded = record_request_usage(
            "acc-a",
            RequestLogUsage {
                total_tokens: Some(321),
                input_tokens: Some(1),
                output_tokens: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(recorded, 321);
        assert_eq!(local_burn_score("acc-a"), 321);

        let recorded = record_request_usage(
            "acc-b",
            RequestLogUsage {
                input_tokens: Some(12),
                output_tokens: Some(34),
                ..Default::default()
            },
        );
        assert_eq!(recorded, 46);
        assert_eq!(local_burn_score("acc-b"), 46);

        let _estimate = EnvGuard::set(ENV_ROUTE_BURN_ESTIMATE_TOKENS, "777");
        reload_from_env();
        let recorded = record_request_usage("acc-c", RequestLogUsage::default());
        assert_eq!(recorded, 777);
        assert_eq!(local_burn_score("acc-c"), 777);
    }

    #[test]
    fn incomplete_stream_requires_repeat_and_success_clears_strikes() {
        let _threshold = EnvGuard::set(ENV_ROUTE_STREAM_INCOMPLETE_THRESHOLD, "2");
        reload_from_env();
        clear_runtime_state_for_tests();

        assert!(!record_stream_incomplete_unknown("acc-stream"));
        assert!(record_stream_incomplete_unknown("acc-stream"));

        let recorded = record_request_usage(
            "acc-stream",
            RequestLogUsage {
                total_tokens: Some(50),
                ..Default::default()
            },
        );
        assert_eq!(recorded, 50);
        assert!(!record_stream_incomplete_unknown("acc-stream"));
    }
}
