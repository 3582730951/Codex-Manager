use super::*;

#[test]
fn lookup_evicts_expired_target_entry_without_full_scan() {
    let _guard = cooldown_test_guard();
    clear_account_cooldown_for_tests();
    let lock = ACCOUNT_COOLDOWN_UNTIL.get_or_init(|| Mutex::new(AccountCooldownState::default()));
    let mut state = lock.lock().expect("cooldown state lock");
    let now = now_ts();
    state.entries.insert("acc-a".to_string(), now - 1);
    state.entries.insert("acc-b".to_string(), now - 1);
    drop(state);

    assert!(!is_account_in_cooldown("acc-a"));

    let state = lock.lock().expect("cooldown state lock");
    assert!(!state.entries.contains_key("acc-a"));
    assert!(state.entries.contains_key("acc-b"));
}

#[test]
fn mark_path_cleanup_prunes_expired_entries() {
    let _guard = cooldown_test_guard();
    clear_account_cooldown_for_tests();
    let lock = ACCOUNT_COOLDOWN_UNTIL.get_or_init(|| Mutex::new(AccountCooldownState::default()));
    let mut state = lock.lock().expect("cooldown state lock");
    let now = now_ts();
    state.entries.insert("stale".to_string(), now - 1);
    state.last_cleanup_at = now - ACCOUNT_COOLDOWN_CLEANUP_INTERVAL_SECS - 1;
    drop(state);

    mark_account_cooldown("fresh", CooldownReason::Default);

    let state = lock.lock().expect("cooldown state lock");
    assert!(!state.entries.contains_key("stale"));
    assert!(state.entries.contains_key("fresh"));
}

#[test]
fn rate_limit_ladder_maps_to_expected_steps() {
    assert_eq!(rate_limit_cooldown_secs_for_offense(1), 45);
    assert_eq!(rate_limit_cooldown_secs_for_offense(2), 300);
    assert_eq!(rate_limit_cooldown_secs_for_offense(3), 1800);
    assert_eq!(rate_limit_cooldown_secs_for_offense(4), 7200);
    assert_eq!(rate_limit_cooldown_secs_for_offense(5), 7200);
}

#[test]
fn rate_limited_mark_increments_and_success_clear_decays_offense() {
    let _guard = cooldown_test_guard();
    clear_account_cooldown_for_tests();
    let lock = ACCOUNT_COOLDOWN_UNTIL.get_or_init(|| Mutex::new(AccountCooldownState::default()));
    mark_account_cooldown("acc", CooldownReason::RateLimited);
    {
        let state = lock.lock().expect("cooldown state lock");
        assert_eq!(state.offense_counts.get("acc"), Some(&1));
    }

    mark_account_cooldown("acc", CooldownReason::RateLimited);
    {
        let state = lock.lock().expect("cooldown state lock");
        assert_eq!(state.offense_counts.get("acc"), Some(&2));
    }

    clear_account_cooldown("acc");
    {
        let state = lock.lock().expect("cooldown state lock");
        assert_eq!(state.offense_counts.get("acc"), Some(&1));
    }

    clear_account_cooldown("acc");
    {
        let state = lock.lock().expect("cooldown state lock");
        assert!(!state.offense_counts.contains_key("acc"));
    }
}

#[test]
fn non_rate_limited_mark_keeps_existing_behavior_without_offense_count() {
    let _guard = cooldown_test_guard();
    clear_account_cooldown_for_tests();
    let lock = ACCOUNT_COOLDOWN_UNTIL.get_or_init(|| Mutex::new(AccountCooldownState::default()));
    mark_account_cooldown("acc", CooldownReason::Default);

    let state = lock.lock().expect("cooldown state lock");
    assert!(state.entries.contains_key("acc"));
    assert!(!state.offense_counts.contains_key("acc"));
}

#[test]
fn rate_limited_offense_resets_after_quiet_period() {
    let _guard = cooldown_test_guard();
    clear_account_cooldown_for_tests();
    let lock = ACCOUNT_COOLDOWN_UNTIL.get_or_init(|| Mutex::new(AccountCooldownState::default()));
    let now = now_ts();
    {
        let mut state = lock.lock().expect("cooldown state lock");
        state.offense_counts.insert("acc".to_string(), 3);
        state.offense_last_at.insert(
            "acc".to_string(),
            now - ACCOUNT_RATE_LIMIT_OFFENSE_FORGET_AFTER_SECS - 1,
        );
    }

    mark_account_cooldown("acc", CooldownReason::RateLimited);

    let state = lock.lock().expect("cooldown state lock");
    assert_eq!(state.offense_counts.get("acc"), Some(&1));
}
