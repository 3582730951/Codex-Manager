use codexmanager_core::storage::{Account, Storage, Token};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in super::super) enum CandidateSkipReason {
    Cooldown,
    Inflight,
}

pub(crate) fn prepare_gateway_candidates(
    storage: &Storage,
    _request_model: Option<&str>,
) -> Result<Vec<(Account, Token)>, String> {
    // 中文注释：保持账号原始顺序（按账户排序字段）作为候选顺序，失败时再依次切下一个。
    super::super::super::collect_gateway_candidates(storage)
}

pub(in super::super) fn free_account_model_override(
    storage: &Storage,
    account: &Account,
    token: &Token,
) -> Option<String> {
    if !crate::account_plan::is_free_or_single_window_account(storage, account.id.as_str(), token) {
        return None;
    }
    let configured = super::super::super::current_free_account_max_model();
    if configured.eq_ignore_ascii_case("auto") {
        None
    } else {
        Some(configured)
    }
}

pub(in super::super) fn candidate_skip_reason_for_proxy(
    account_id: &str,
    idx: usize,
    candidate_count: usize,
    dynamic_account_limit: usize,
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

    let has_more_candidates = idx + 1 < candidate_count;
    if super::super::super::scheduler_account_in_cooldown(account_id) && has_more_candidates {
        super::super::super::record_gateway_failover_attempt();
        return Some(CandidateSkipReason::Cooldown);
    }

    if dynamic_account_limit > 0
        && super::super::super::scheduler_account_inflight_count(account_id)
            >= dynamic_account_limit
    {
        super::super::super::record_gateway_failover_attempt();
        return Some(CandidateSkipReason::Inflight);
    }

    None
}
#[cfg(test)]
mod tests {
    use super::free_account_model_override;
    use codexmanager_core::storage::{now_ts, Account, Storage, Token, UsageSnapshotRecord};

    #[test]
    fn free_account_model_override_uses_configured_model_for_free_account() {
        let _runtime_guard = crate::gateway::gateway_runtime_test_guard();
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
        let _runtime_guard = crate::gateway::gateway_runtime_test_guard();
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
    fn free_account_model_override_skips_rewrite_when_configured_auto() {
        let _runtime_guard = crate::gateway::gateway_runtime_test_guard();
        let storage = Storage::open_in_memory().expect("open");
        storage.init().expect("init");
        let now = now_ts();
        storage
            .insert_account(&Account {
                id: "acc-auto".to_string(),
                label: "acc-auto".to_string(),
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
            account_id: "acc-auto".to_string(),
            id_token: "header.payload.sig".to_string(),
            access_token: "header.payload.sig".to_string(),
            refresh_token: "refresh".to_string(),
            api_key_access_token: None,
            last_refresh: now,
        };
        storage.insert_token(&token).expect("insert token");
        storage
            .insert_usage_snapshot(&UsageSnapshotRecord {
                account_id: "acc-auto".to_string(),
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
        crate::gateway::set_free_account_max_model("auto").expect("set free model");

        let account = Account {
            id: "acc-auto".to_string(),
            label: "acc-auto".to_string(),
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

        assert_eq!(actual, None);
    }
}
