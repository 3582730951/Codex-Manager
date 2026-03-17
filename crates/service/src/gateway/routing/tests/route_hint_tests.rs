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

    fn clear(key: &'static str) -> Self {
        let original = std::env::var_os(key);
        std::env::remove_var(key);
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

fn candidate_list() -> Vec<(Account, Token)> {
    vec![
        (
            Account {
                id: "acc-a".to_string(),
                label: "".to_string(),
                issuer: "".to_string(),
                chatgpt_account_id: None,
                workspace_id: None,
                group_name: None,
                sort: 0,
                status: "active".to_string(),
                created_at: 0,
                updated_at: 0,
            },
            Token {
                account_id: "acc-a".to_string(),
                id_token: "".to_string(),
                access_token: "".to_string(),
                refresh_token: "".to_string(),
                api_key_access_token: None,
                last_refresh: 0,
            },
        ),
        (
            Account {
                id: "acc-b".to_string(),
                label: "".to_string(),
                issuer: "".to_string(),
                chatgpt_account_id: None,
                workspace_id: None,
                group_name: None,
                sort: 1,
                status: "active".to_string(),
                created_at: 0,
                updated_at: 0,
            },
            Token {
                account_id: "acc-b".to_string(),
                id_token: "".to_string(),
                access_token: "".to_string(),
                refresh_token: "".to_string(),
                api_key_access_token: None,
                last_refresh: 0,
            },
        ),
        (
            Account {
                id: "acc-c".to_string(),
                label: "".to_string(),
                issuer: "".to_string(),
                chatgpt_account_id: None,
                workspace_id: None,
                group_name: None,
                sort: 2,
                status: "active".to_string(),
                created_at: 0,
                updated_at: 0,
            },
            Token {
                account_id: "acc-c".to_string(),
                id_token: "".to_string(),
                access_token: "".to_string(),
                refresh_token: "".to_string(),
                api_key_access_token: None,
                last_refresh: 0,
            },
        ),
    ]
}

fn account_ids(candidates: &[(Account, Token)]) -> Vec<String> {
    candidates
        .iter()
        .map(|(account, _)| account.id.clone())
        .collect()
}

fn selection(flow_key: &'static str) -> RouteSelectionContext<'static> {
    RouteSelectionContext::new("gk_1", Some("gpt-5.3-codex"), flow_key)
}

#[test]
fn defaults_to_balanced_strategy() {
    let _guard = route_strategy_test_guard();
    let _route_strategy = EnvGuard::clear(ROUTE_STRATEGY_ENV);
    let _instance = EnvGuard::set("CODEXMANAGER_INSTANCE_ID", "inst-route-default");
    reload_from_env();
    clear_route_state_for_tests();

    let mut first = candidate_list();
    apply_route_strategy(&mut first, selection("flow-stable-1"));

    let mut second = candidate_list();
    apply_route_strategy(&mut second, selection("flow-stable-1"));

    assert_eq!(account_ids(&first), account_ids(&second));
}

#[test]
fn balanced_strategy_changes_head_when_flow_key_changes() {
    let _guard = route_strategy_test_guard();
    let _route_strategy = EnvGuard::set(ROUTE_STRATEGY_ENV, "balanced");
    let _instance = EnvGuard::set("CODEXMANAGER_INSTANCE_ID", "inst-route-flow");
    reload_from_env();
    clear_route_state_for_tests();

    let mut heads = std::collections::BTreeSet::new();
    for flow_key in ["flow-a", "flow-b", "flow-c", "flow-d"] {
        let mut candidates = candidate_list();
        apply_route_strategy(&mut candidates, selection(flow_key));
        heads.insert(account_ids(&candidates)[0].clone());
    }

    assert!(heads.len() > 1);
}

#[test]
fn balanced_prefers_lower_burn_within_rendezvous_window() {
    let _guard = route_strategy_test_guard();
    let _route_strategy = EnvGuard::set(ROUTE_STRATEGY_ENV, "balanced");
    let _window = EnvGuard::set(ROUTE_RENDEZVOUS_TOP_K_ENV, "3");
    let _instance = EnvGuard::set("CODEXMANAGER_INSTANCE_ID", "inst-route-burn");
    reload_from_env();
    clear_route_state_for_tests();

    super::super::local_burn::record_request_usage(
        "acc-a",
        super::super::request_log::RequestLogUsage {
            total_tokens: Some(10_000),
            ..Default::default()
        },
    );
    super::super::local_burn::record_request_usage(
        "acc-b",
        super::super::request_log::RequestLogUsage {
            total_tokens: Some(2_000),
            ..Default::default()
        },
    );

    let mut candidates = candidate_list();
    apply_route_strategy(&mut candidates, selection("flow-burn"));

    assert_eq!(account_ids(&candidates)[0], "acc-c");
}

#[test]
fn balanced_filters_cooldown_and_inflight_before_selection() {
    let _guard = route_strategy_test_guard();
    let _route_strategy = EnvGuard::set(ROUTE_STRATEGY_ENV, "balanced");
    let _instance = EnvGuard::set("CODEXMANAGER_INSTANCE_ID", "inst-route-filter");
    reload_from_env();
    clear_route_state_for_tests();
    super::super::cooldown::mark_account_cooldown(
        "acc-a",
        super::super::cooldown::CooldownReason::RateLimited,
    );
    let inflight_guard = super::super::metrics::acquire_account_inflight("acc-b");

    let mut candidates = candidate_list();
    apply_route_strategy(&mut candidates, selection("flow-filter"));

    drop(inflight_guard);
    super::super::metrics::clear_account_inflight_for_tests();
    super::super::cooldown::clear_runtime_state();

    assert_eq!(account_ids(&candidates), vec!["acc-c".to_string()]);
}

#[test]
fn ordered_only_reorders_within_small_prefix_window() {
    let _guard = route_strategy_test_guard();
    let _route_strategy = EnvGuard::set(ROUTE_STRATEGY_ENV, "ordered");
    let _window = EnvGuard::set(ROUTE_ORDERED_PREFIX_WINDOW_ENV, "2");
    let _instance = EnvGuard::set("CODEXMANAGER_INSTANCE_ID", "inst-route-ordered");
    reload_from_env();
    clear_route_state_for_tests();

    let mut candidates = candidate_list();
    apply_route_strategy(&mut candidates, selection("flow-ordered"));

    let ids = account_ids(&candidates);
    assert_ne!(ids[0], "acc-c");
    assert_eq!(ids[2], "acc-c");
}

#[test]
fn instance_id_salt_changes_balanced_head_across_instances() {
    let _guard = route_strategy_test_guard();
    let _route_strategy = EnvGuard::set(ROUTE_STRATEGY_ENV, "balanced");
    reload_from_env();
    clear_route_state_for_tests();

    let mut heads = std::collections::BTreeSet::new();
    for instance_id in ["inst-a", "inst-b", "inst-c", "inst-d"] {
        let _instance = EnvGuard::set("CODEXMANAGER_INSTANCE_ID", instance_id);
        super::super::instance_id::reload_from_env();
        let mut candidates = candidate_list();
        apply_route_strategy(&mut candidates, selection("same-flow"));
        heads.insert(account_ids(&candidates)[0].clone());
    }

    assert!(heads.len() > 1);
}

#[test]
fn balanced_multi_instance_simulation_spreads_heads_without_single_hotspot() {
    let _guard = route_strategy_test_guard();
    let _route_strategy = EnvGuard::set(ROUTE_STRATEGY_ENV, "balanced");
    let _window = EnvGuard::set(ROUTE_RENDEZVOUS_TOP_K_ENV, "2");
    reload_from_env();
    clear_route_state_for_tests();

    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for instance_idx in 0..8 {
        let instance_id = format!("inst-sim-{instance_idx}");
        let _instance = EnvGuard::set("CODEXMANAGER_INSTANCE_ID", instance_id.as_str());
        super::super::instance_id::reload_from_env();
        for flow_idx in 0..240 {
            let flow_key = format!("flow-sim-{flow_idx}");
            let mut candidates = candidate_list();
            apply_route_strategy(
                &mut candidates,
                RouteSelectionContext::new("gk_1", Some("gpt-5.3-codex"), flow_key.as_str()),
            );
            *counts
                .entry(account_ids(&candidates)[0].clone())
                .or_default() += 1;
        }
    }

    let min = counts.values().copied().min().expect("min head count");
    let max = counts.values().copied().max().expect("max head count");
    assert_eq!(counts.len(), 3, "counts={counts:?}");
    assert!(min >= 400, "counts={counts:?}");
    assert!(max - min <= 240, "counts={counts:?}");
}

#[test]
fn set_route_strategy_accepts_aliases_and_reports_canonical_name() {
    let _guard = route_strategy_test_guard();
    clear_route_state_for_tests();
    assert_eq!(
        set_route_strategy("ordered").expect("set ordered"),
        "ordered"
    );
    assert_eq!(
        set_route_strategy("round_robin").expect("set rr alias"),
        "balanced"
    );
    assert_eq!(current_route_strategy(), "balanced");
    assert!(set_route_strategy("unsupported").is_err());
}

#[test]
fn manual_preferred_account_is_preserved_when_current_candidates_do_not_include_it() {
    let _guard = route_strategy_test_guard();
    clear_route_state_for_tests();

    let mut expected = candidate_list();
    apply_route_strategy(&mut expected, selection("flow-manual"));

    set_manual_preferred_account("acc-missing").expect("set manual preferred");

    let mut candidates = candidate_list();
    apply_route_strategy(&mut candidates, selection("flow-manual"));

    assert_eq!(
        get_manual_preferred_account().as_deref(),
        Some("acc-missing")
    );
    assert_eq!(account_ids(&candidates), account_ids(&expected));
}
