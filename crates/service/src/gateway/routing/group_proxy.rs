use codexmanager_core::storage::{now_ts, Account};
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

const GROUP_PROXY_SAMPLE_WINDOW: usize = 24;
const GROUP_PROXY_MIN_SAMPLES: usize = 6;
const GROUP_PROXY_CHALLENGE_THRESHOLD_PERCENT: usize = 35;
const GROUP_PROXY_WARP_HOLD_SECS: i64 = 15 * 60;
const GROUP_PROXY_TTL_SECS: i64 = 24 * 60 * 60;
const GROUP_PROXY_CLEANUP_INTERVAL_SECS: i64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupProxyPolicyMode {
    AlwaysWarp,
    AlwaysDirect,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GroupProxyPolicy {
    scope: String,
    mode: GroupProxyPolicyMode,
}

#[derive(Debug, Clone, Default)]
struct GroupProxyRiskRecord {
    observations: VecDeque<bool>,
    warp_until: i64,
    updated_at: i64,
}

#[derive(Default)]
struct GroupProxyRiskState {
    entries: HashMap<String, GroupProxyRiskRecord>,
    last_cleanup_at: i64,
}

static GROUP_PROXY_STATE: OnceLock<Mutex<GroupProxyRiskState>> = OnceLock::new();

fn normalize_scope(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

fn fallback_scope(account: &Account) -> String {
    account
        .workspace_id
        .as_deref()
        .and_then(normalize_scope)
        .or_else(|| {
            account
                .chatgpt_account_id
                .as_deref()
                .and_then(normalize_scope)
        })
        .or_else(|| normalize_scope(account.id.as_str()))
        .unwrap_or_else(|| "default".to_string())
}

fn parse_prefixed_group_policy(raw: &str) -> Option<(GroupProxyPolicyMode, Option<String>)> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("warp") {
        return Some((GroupProxyPolicyMode::AlwaysWarp, None));
    }
    if trimmed.eq_ignore_ascii_case("direct") {
        return Some((GroupProxyPolicyMode::AlwaysDirect, None));
    }
    if trimmed.eq_ignore_ascii_case("auto") {
        return Some((GroupProxyPolicyMode::Auto, None));
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("warp:") {
        return Some((GroupProxyPolicyMode::AlwaysWarp, normalize_scope(&trimmed[5..])));
    }
    if lower.starts_with("direct:") {
        return Some((GroupProxyPolicyMode::AlwaysDirect, normalize_scope(&trimmed[7..])));
    }
    if lower.starts_with("auto:") {
        return Some((GroupProxyPolicyMode::Auto, normalize_scope(&trimmed[5..])));
    }
    None
}

fn resolve_group_proxy_policy(account: &Account) -> GroupProxyPolicy {
    let current_mode = super::runtime_config::gateway_account_proxy_mode();
    let fallback = fallback_scope(account);
    let raw_group = account
        .group_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(raw_group) = raw_group {
        if let Some((mode, explicit_scope)) = parse_prefixed_group_policy(raw_group) {
            return GroupProxyPolicy {
                scope: explicit_scope.unwrap_or_else(|| fallback.clone()),
                mode,
            };
        }
        return GroupProxyPolicy {
            scope: normalize_scope(raw_group).unwrap_or(fallback),
            mode: match current_mode {
                super::runtime_config::GatewayAccountProxyMode::Always => {
                    GroupProxyPolicyMode::AlwaysWarp
                }
                super::runtime_config::GatewayAccountProxyMode::GroupAuto => {
                    GroupProxyPolicyMode::Auto
                }
            },
        };
    }

    GroupProxyPolicy {
        scope: fallback,
        mode: match current_mode {
            super::runtime_config::GatewayAccountProxyMode::Always => {
                GroupProxyPolicyMode::AlwaysWarp
            }
            super::runtime_config::GatewayAccountProxyMode::GroupAuto => {
                GroupProxyPolicyMode::Auto
            }
        },
    }
}

fn challenge_count(record: &GroupProxyRiskRecord) -> usize {
    record
        .observations
        .iter()
        .copied()
        .filter(|is_challenge| *is_challenge)
        .count()
}

fn challenge_ratio_triggers_warp(record: &GroupProxyRiskRecord) -> bool {
    let sample_count = record.observations.len();
    if sample_count < GROUP_PROXY_MIN_SAMPLES {
        return false;
    }
    challenge_count(record) * 100 >= GROUP_PROXY_CHALLENGE_THRESHOLD_PERCENT * sample_count
}

fn maybe_cleanup_expired_entries(state: &mut GroupProxyRiskState, now: i64) {
    if state.last_cleanup_at != 0
        && now.saturating_sub(state.last_cleanup_at) < GROUP_PROXY_CLEANUP_INTERVAL_SECS
    {
        return;
    }
    state.last_cleanup_at = now;
    state.entries.retain(|_, record| {
        record.updated_at.saturating_add(GROUP_PROXY_TTL_SECS) > now || record.warp_until > now
    });
}

fn record_group_observation(scope: &str, is_challenge: bool) {
    let lock = GROUP_PROXY_STATE.get_or_init(|| Mutex::new(GroupProxyRiskState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "group_proxy_state");
    let now = now_ts();
    maybe_cleanup_expired_entries(&mut state, now);
    let record = state.entries.entry(scope.to_string()).or_default();
    record.updated_at = now;
    if record.observations.len() >= GROUP_PROXY_SAMPLE_WINDOW {
        record.observations.pop_front();
    }
    record.observations.push_back(is_challenge);
    if is_challenge && challenge_ratio_triggers_warp(record) {
        let next_warp_until = now.saturating_add(GROUP_PROXY_WARP_HOLD_SECS);
        let extended = next_warp_until > record.warp_until;
        if extended {
            record.warp_until = next_warp_until;
            let sample_count = record.observations.len();
            let challenges = challenge_count(record);
            log::warn!(
                "event=gateway_group_proxy_warp_hold scope={} challenges={} samples={} hold_secs={}",
                scope,
                challenges,
                sample_count,
                GROUP_PROXY_WARP_HOLD_SECS
            );
        }
    }
}

fn is_group_in_warp_hold(scope: &str) -> bool {
    let lock = GROUP_PROXY_STATE.get_or_init(|| Mutex::new(GroupProxyRiskState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "group_proxy_state");
    let now = now_ts();
    maybe_cleanup_expired_entries(&mut state, now);
    match state.entries.get(scope).cloned() {
        Some(record) if record.warp_until > now => true,
        Some(record) if record.updated_at.saturating_add(GROUP_PROXY_TTL_SECS) <= now => {
            state.entries.remove(scope);
            false
        }
        _ => false,
    }
}

pub(crate) fn should_use_gateway_account_proxy_for_account(account: &Account) -> bool {
    if super::runtime_config::gateway_account_proxy_url().is_none() {
        return false;
    }
    let policy = resolve_group_proxy_policy(account);
    match policy.mode {
        GroupProxyPolicyMode::AlwaysWarp => true,
        GroupProxyPolicyMode::AlwaysDirect => false,
        GroupProxyPolicyMode::Auto => is_group_in_warp_hold(policy.scope.as_str()),
    }
}

pub(crate) fn record_account_group_proxy_challenge(account: &Account) {
    let policy = resolve_group_proxy_policy(account);
    if policy.mode == GroupProxyPolicyMode::Auto {
        record_group_observation(policy.scope.as_str(), true);
    }
}

pub(crate) fn record_account_group_proxy_success(account: &Account) {
    let policy = resolve_group_proxy_policy(account);
    if policy.mode == GroupProxyPolicyMode::Auto {
        record_group_observation(policy.scope.as_str(), false);
    }
}

pub(super) fn clear_runtime_state() {
    let lock = GROUP_PROXY_STATE.get_or_init(|| Mutex::new(GroupProxyRiskState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "group_proxy_state");
    state.entries.clear();
    state.last_cleanup_at = 0;
}

#[cfg(test)]
mod tests {
    use super::{clear_runtime_state, resolve_group_proxy_policy, should_use_gateway_account_proxy_for_account};
    use codexmanager_core::storage::Account;
    use std::sync::MutexGuard;

    fn runtime_guard() -> MutexGuard<'static, ()> {
        crate::gateway::gateway_runtime_test_guard()
    }

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
            crate::gateway::reload_runtime_config_from_env();
        }
    }

    fn build_account(group_name: Option<&str>) -> Account {
        Account {
            id: "acc-1".to_string(),
            label: "acc-1".to_string(),
            issuer: "issuer".to_string(),
            chatgpt_account_id: Some("chatgpt-1".to_string()),
            workspace_id: Some("workspace-1".to_string()),
            group_name: group_name.map(str::to_string),
            sort: 0,
            status: "active".to_string(),
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn explicit_warp_group_always_uses_proxy() {
        let _guard = runtime_guard();
        let _proxy = EnvGuard::set("CODEXMANAGER_GATEWAY_ACCOUNT_PROXY_URL", "socks5://127.0.0.1:40000");
        crate::gateway::reload_runtime_config_from_env();
        clear_runtime_state();

        let account = build_account(Some("warp:team-a"));

        assert!(should_use_gateway_account_proxy_for_account(&account));
    }

    #[test]
    fn explicit_direct_group_never_uses_proxy() {
        let _guard = runtime_guard();
        let _proxy = EnvGuard::set("CODEXMANAGER_GATEWAY_ACCOUNT_PROXY_URL", "socks5://127.0.0.1:40000");
        crate::gateway::reload_runtime_config_from_env();
        clear_runtime_state();

        let account = build_account(Some("direct:team-a"));

        assert!(!should_use_gateway_account_proxy_for_account(&account));
    }

    #[test]
    fn auto_group_enters_warp_hold_after_frequent_challenge() {
        let _guard = runtime_guard();
        let _proxy = EnvGuard::set("CODEXMANAGER_GATEWAY_ACCOUNT_PROXY_URL", "socks5://127.0.0.1:40000");
        let _mode = EnvGuard::set("CODEXMANAGER_GATEWAY_ACCOUNT_PROXY_MODE", "group_auto");
        crate::gateway::reload_runtime_config_from_env();
        clear_runtime_state();

        let account = build_account(Some("auto:team-a"));
        for _ in 0..3 {
            super::record_account_group_proxy_success(&account);
        }
        for _ in 0..3 {
            super::record_account_group_proxy_challenge(&account);
        }
        assert!(should_use_gateway_account_proxy_for_account(&account));
    }

    #[test]
    fn plain_group_respects_group_auto_mode() {
        let _guard = runtime_guard();
        let _mode = EnvGuard::set("CODEXMANAGER_GATEWAY_ACCOUNT_PROXY_MODE", "group_auto");
        crate::gateway::reload_runtime_config_from_env();
        clear_runtime_state();

        let policy = resolve_group_proxy_policy(&build_account(Some("TEAM-A")));

        assert_eq!(policy.scope, "team-a");
        assert_eq!(policy.mode, super::GroupProxyPolicyMode::Auto);
    }
}
