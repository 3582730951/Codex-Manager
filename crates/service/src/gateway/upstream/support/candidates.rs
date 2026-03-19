use codexmanager_core::storage::{now_ts, Account, Storage, Token, UsageSnapshotRecord};
use std::cmp::Reverse;
use std::collections::HashMap;

const STALE_USAGE_SNAPSHOT_SECS: i64 = 30 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in super::super) enum CandidateSkipReason {
    Cooldown,
    Inflight,
}

pub(crate) fn prepare_gateway_candidates(
    storage: &Storage,
    request_model: Option<&str>,
) -> Result<Vec<(Account, Token)>, String> {
    let candidates = super::super::super::collect_gateway_candidates(storage)?;
    if candidates.len() <= 1 {
        return Ok(candidates);
    }

    let usage_by_account = storage
        .latest_usage_snapshots_by_account()
        .map_err(|err| err.to_string())?
        .into_iter()
        .map(|snapshot| (snapshot.account_id.clone(), snapshot))
        .collect::<HashMap<_, _>>();
    let normalized_request_model = normalize_request_model(request_model);
    let now = now_ts();

    let mut indexed = candidates
        .into_iter()
        .enumerate()
        .collect::<Vec<(usize, (Account, Token))>>();
    indexed.sort_by(|(left_idx, (left_account, left_token)), (right_idx, (right_account, right_token))| {
        let left_priority = candidate_priority(
            storage,
            left_account,
            left_token,
            usage_by_account.get(left_account.id.as_str()),
            normalized_request_model.as_deref(),
            now,
        );
        let right_priority = candidate_priority(
            storage,
            right_account,
            right_token,
            usage_by_account.get(right_account.id.as_str()),
            normalized_request_model.as_deref(),
            now,
        );
        left_priority
            .cmp(&right_priority)
            .then(left_idx.cmp(right_idx))
    });

    Ok(indexed.into_iter().map(|(_, pair)| pair).collect())
}

pub(in super::super) fn free_account_model_override(
    storage: &Storage,
    account: &Account,
    token: &Token,
) -> Option<String> {
    if !crate::account_plan::is_free_or_single_window_account(storage, account.id.as_str(), token) {
        return None;
    }
    Some(super::super::super::current_free_account_max_model())
}

pub(in super::super) fn candidate_skip_reason_for_proxy(
    account_id: &str,
    idx: usize,
    _candidate_count: usize,
    account_max_inflight: usize,
) -> Option<CandidateSkipReason> {
    // 中文注释：当用户手动“切到当前”后，首候选应持续优先命中；
    // 仅在真实请求失败时由上游流程自动清除手动锁定，再回退常规轮转。
    let is_manual_preferred_head = idx == 0
        && super::super::super::manual_preferred_account()
            .as_deref()
            .is_some_and(|manual_id| manual_id == account_id);
    if is_manual_preferred_head {
        return None;
    }

    if super::super::super::is_account_in_cooldown(account_id) {
        super::super::super::record_gateway_failover_attempt();
        return Some(CandidateSkipReason::Cooldown);
    }

    if account_max_inflight > 0
        && super::super::super::account_inflight_count(account_id) >= account_max_inflight
    {
        super::super::super::record_gateway_failover_attempt();
        return Some(CandidateSkipReason::Inflight);
    }

    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CandidatePriority {
    request_feedback_rank: u8,
    requires_model_override: bool,
    missing_snapshot: bool,
    stale_snapshot: bool,
    headroom_rank: Reverse<i64>,
    snapshot_age_secs: i64,
}

fn candidate_priority(
    storage: &Storage,
    account: &Account,
    token: &Token,
    usage_snapshot: Option<&UsageSnapshotRecord>,
    request_model: Option<&str>,
    now: i64,
) -> CandidatePriority {
    let request_feedback_rank = match super::super::super::request_feedback_for(&account.id, request_model) {
        Some(super::super::super::AccountRequestFeedback::ModelIneligible) => 3,
        Some(super::super::super::AccountRequestFeedback::QuotaRejected) => 2,
        None => 0,
    };
    let requires_model_override = request_model
        .zip(free_account_model_override(storage, account, token))
        .is_some_and(|(requested, override_model)| requested != override_model);
    let missing_snapshot = usage_snapshot.is_none();
    let stale_snapshot = usage_snapshot
        .map(|snapshot| now.saturating_sub(snapshot.captured_at) > STALE_USAGE_SNAPSHOT_SECS)
        .unwrap_or(false);
    let headroom_rank = Reverse(snapshot_headroom(usage_snapshot));
    let snapshot_age_secs = usage_snapshot
        .map(|snapshot| now.saturating_sub(snapshot.captured_at).max(0))
        .unwrap_or(i64::MAX);
    CandidatePriority {
        request_feedback_rank,
        requires_model_override,
        missing_snapshot,
        stale_snapshot,
        headroom_rank,
        snapshot_age_secs,
    }
}

fn snapshot_headroom(snapshot: Option<&UsageSnapshotRecord>) -> i64 {
    let Some(snapshot) = snapshot else {
        return -1;
    };
    let primary = percent_headroom(snapshot.used_percent).unwrap_or(0);
    let secondary = percent_headroom(snapshot.secondary_used_percent).unwrap_or(10_000);
    primary.min(secondary)
}

fn percent_headroom(value: Option<f64>) -> Option<i64> {
    value.map(|used| ((100.0 - used.clamp(0.0, 100.0)) * 100.0).round() as i64)
}

fn normalize_request_model(request_model: Option<&str>) -> Option<String> {
    request_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::{
        free_account_model_override, prepare_gateway_candidates, STALE_USAGE_SNAPSHOT_SECS,
    };
    use codexmanager_core::storage::{now_ts, Account, Storage, Token, UsageSnapshotRecord};
    use std::sync::{Mutex, MutexGuard};

    static CANDIDATE_SUPPORT_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn candidate_support_test_guard() -> MutexGuard<'static, ()> {
        CANDIDATE_SUPPORT_TEST_LOCK
            .lock()
            .expect("lock candidate support tests")
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

    #[test]
    fn free_account_model_override_uses_configured_model_for_free_account() {
        let _guard = candidate_support_test_guard();
        let storage = Storage::open_in_memory().expect("open");
        storage.init().expect("init");
        let now = now_ts();
        storage
            .insert_account(&Account {
                id: "acc-free".to_string(),
                label: "acc-free".to_string(),
                issuer: "issuer".to_string(),
                chatgpt_account_id: None,
                workspace_id: None,
                group_name: None,
                sort: 0,
                status: "active".to_string(),
                created_at: now,
                updated_at: now,
            })
            .expect("insert account");
        let token = Token {
            account_id: "acc-free".to_string(),
            id_token: "header.payload.sig".to_string(),
            access_token: "header.payload.sig".to_string(),
            refresh_token: "refresh".to_string(),
            api_key_access_token: None,
            last_refresh: now,
        };
        storage.insert_token(&token).expect("insert token");
        storage
            .insert_usage_snapshot(&UsageSnapshotRecord {
                account_id: "acc-free".to_string(),
                used_percent: Some(10.0),
                window_minutes: Some(300),
                resets_at: None,
                secondary_used_percent: Some(20.0),
                secondary_window_minutes: Some(10_080),
                secondary_resets_at: None,
                credits_json: Some(r#"{"planType":"free"}"#.to_string()),
                captured_at: now,
            })
            .expect("insert usage");

        let original = crate::gateway::current_free_account_max_model();
        crate::gateway::set_free_account_max_model("gpt-5.2").expect("set free model");

        let account = Account {
            id: "acc-free".to_string(),
            label: "acc-free".to_string(),
            issuer: "issuer".to_string(),
            chatgpt_account_id: None,
            workspace_id: None,
            group_name: None,
            sort: 0,
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        };
        let actual = free_account_model_override(&storage, &account, &token);

        let _ = crate::gateway::set_free_account_max_model(&original);

        assert_eq!(actual.as_deref(), Some("gpt-5.2"));
    }

    #[test]
    fn free_account_model_override_accepts_single_window_weekly_account() {
        let _guard = candidate_support_test_guard();
        let storage = Storage::open_in_memory().expect("open");
        storage.init().expect("init");
        let now = now_ts();
        storage
            .insert_account(&Account {
                id: "acc-weekly".to_string(),
                label: "acc-weekly".to_string(),
                issuer: "issuer".to_string(),
                chatgpt_account_id: None,
                workspace_id: None,
                group_name: None,
                sort: 0,
                status: "active".to_string(),
                created_at: now,
                updated_at: now,
            })
            .expect("insert account");
        let token = Token {
            account_id: "acc-weekly".to_string(),
            id_token: "header.payload.sig".to_string(),
            access_token: "header.payload.sig".to_string(),
            refresh_token: "refresh".to_string(),
            api_key_access_token: None,
            last_refresh: now,
        };
        storage.insert_token(&token).expect("insert token");
        storage
            .insert_usage_snapshot(&UsageSnapshotRecord {
                account_id: "acc-weekly".to_string(),
                used_percent: Some(10.0),
                window_minutes: Some(10_080),
                resets_at: None,
                secondary_used_percent: None,
                secondary_window_minutes: None,
                secondary_resets_at: None,
                credits_json: None,
                captured_at: now,
            })
            .expect("insert usage");

        let original = crate::gateway::current_free_account_max_model();
        crate::gateway::set_free_account_max_model("gpt-5.2").expect("set free model");

        let account = Account {
            id: "acc-weekly".to_string(),
            label: "acc-weekly".to_string(),
            issuer: "issuer".to_string(),
            chatgpt_account_id: None,
            workspace_id: None,
            group_name: None,
            sort: 0,
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        };
        let actual = free_account_model_override(&storage, &account, &token);

        let _ = crate::gateway::set_free_account_max_model(&original);

        assert_eq!(actual.as_deref(), Some("gpt-5.2"));
    }

    #[test]
    fn prepare_gateway_candidates_prefers_accounts_with_fresh_headroom() {
        let _guard = candidate_support_test_guard();
        let _db_path = EnvGuard::set("CODEXMANAGER_DB_PATH", "support-candidates-fresh");
        crate::gateway::reload_runtime_config_from_env();
        crate::gateway::clear_request_feedback_runtime_state_for_tests();
        let storage = Storage::open_in_memory().expect("open");
        storage.init().expect("init");
        let now = now_ts();
        for (id, sort, used_percent, captured_at) in [
            ("acc-missing", 0_i64, None, None),
            ("acc-stale", 1_i64, Some(12.0), Some(now - (STALE_USAGE_SNAPSHOT_SECS + 60))),
            ("acc-fresh", 2_i64, Some(8.0), Some(now)),
        ] {
            storage
                .insert_account(&Account {
                    id: id.to_string(),
                    label: id.to_string(),
                    issuer: "issuer".to_string(),
                    chatgpt_account_id: None,
                    workspace_id: None,
                    group_name: None,
                    sort,
                    status: "active".to_string(),
                    created_at: now,
                    updated_at: now,
                })
                .expect("insert account");
            storage
                .insert_token(&Token {
                    account_id: id.to_string(),
                    id_token: "id".to_string(),
                    access_token: "access".to_string(),
                    refresh_token: "refresh".to_string(),
                    api_key_access_token: None,
                    last_refresh: now,
                })
                .expect("insert token");
            if let (Some(used_percent), Some(captured_at)) = (used_percent, captured_at) {
                storage
                    .insert_usage_snapshot(&UsageSnapshotRecord {
                        account_id: id.to_string(),
                        used_percent: Some(used_percent),
                        window_minutes: Some(300),
                        resets_at: None,
                        secondary_used_percent: None,
                        secondary_window_minutes: None,
                        secondary_resets_at: None,
                        credits_json: None,
                        captured_at,
                    })
                    .expect("insert usage");
            }
        }

        let candidates =
            prepare_gateway_candidates(&storage, Some("gpt-5.3-codex")).expect("candidates");
        let ordered = candidates
            .iter()
            .map(|(account, _)| account.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ordered, vec!["acc-fresh", "acc-stale", "acc-missing"]);
    }

    #[test]
    fn prepare_gateway_candidates_deprioritizes_recent_model_rejection() {
        let _guard = candidate_support_test_guard();
        let _db_path = EnvGuard::set("CODEXMANAGER_DB_PATH", "support-candidates-feedback");
        crate::gateway::reload_runtime_config_from_env();
        crate::gateway::clear_request_feedback_runtime_state_for_tests();
        let storage = Storage::open_in_memory().expect("open");
        storage.init().expect("init");
        let now = now_ts();
        for id in ["acc-a", "acc-b"] {
            storage
                .insert_account(&Account {
                    id: id.to_string(),
                    label: id.to_string(),
                    issuer: "issuer".to_string(),
                    chatgpt_account_id: None,
                    workspace_id: None,
                    group_name: None,
                    sort: 0,
                    status: "active".to_string(),
                    created_at: now,
                    updated_at: now,
                })
                .expect("insert account");
            storage
                .insert_token(&Token {
                    account_id: id.to_string(),
                    id_token: "id".to_string(),
                    access_token: "access".to_string(),
                    refresh_token: "refresh".to_string(),
                    api_key_access_token: None,
                    last_refresh: now,
                })
                .expect("insert token");
            storage
                .insert_usage_snapshot(&UsageSnapshotRecord {
                    account_id: id.to_string(),
                    used_percent: Some(10.0),
                    window_minutes: Some(300),
                    resets_at: None,
                    secondary_used_percent: None,
                    secondary_window_minutes: None,
                    secondary_resets_at: None,
                    credits_json: None,
                    captured_at: now,
                })
                .expect("insert usage");
        }

        crate::gateway::record_model_ineligible_feedback("acc-a", "gpt-5.4");
        let candidates = prepare_gateway_candidates(&storage, Some("gpt-5.4")).expect("candidates");
        let ordered = candidates
            .iter()
            .map(|(account, _)| account.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ordered, vec!["acc-b", "acc-a"]);
    }
}
