use super::*;

#[test]
fn same_scope_reuses_same_lock_instance() {
    let _guard = request_gate_test_guard();
    clear_request_gate_locks_for_tests();
    let first = request_gate_lock("gk_1", "/v1/responses", Some("gpt-5.3-codex"));
    let second = request_gate_lock("gk_1", "/v1/responses", Some("gpt-5.3-codex"));
    assert!(Arc::ptr_eq(&first, &second));
}

#[test]
fn different_scope_uses_different_lock_instances() {
    let _guard = request_gate_test_guard();
    clear_request_gate_locks_for_tests();
    let first = request_gate_lock("gk_1", "/v1/responses", Some("gpt-5.3-codex"));
    let second = request_gate_lock("gk_1", "/v1/responses", Some("gpt-5.3-codex-high"));
    assert!(!Arc::ptr_eq(&first, &second));
}

#[test]
fn stale_unshared_lock_entry_is_reclaimed() {
    let _guard = request_gate_test_guard();
    clear_request_gate_locks_for_tests();
    let key = gate_key("gk_1", "/v1/responses", Some("gpt-5.3-codex"));
    let first = request_gate_lock("gk_1", "/v1/responses", Some("gpt-5.3-codex"));
    let weak = Arc::downgrade(&first);
    drop(first);

    let lock = REQUEST_GATE_LOCKS.get_or_init(|| Mutex::new(RequestGateLockTable::default()));
    let mut table = lock.lock().expect("request gate table lock");
    let now = now_ts();
    table
        .entries
        .get_mut(&key)
        .expect("request gate entry")
        .last_seen_at = now - REQUEST_GATE_LOCK_TTL_SECS - 1;
    table.last_cleanup_at = now - REQUEST_GATE_LOCK_CLEANUP_INTERVAL_SECS - 1;
    drop(table);

    let _second = request_gate_lock("gk_1", "/v1/responses", Some("gpt-5.3-codex"));
    assert!(weak.upgrade().is_none());
}

#[test]
fn stale_shared_lock_entry_is_not_reclaimed() {
    let _guard = request_gate_test_guard();
    clear_request_gate_locks_for_tests();
    let key = gate_key("gk_1", "/v1/responses", Some("gpt-5.3-codex"));
    let first = request_gate_lock("gk_1", "/v1/responses", Some("gpt-5.3-codex"));

    let lock = REQUEST_GATE_LOCKS.get_or_init(|| Mutex::new(RequestGateLockTable::default()));
    let mut table = lock.lock().expect("request gate table lock");
    let now = now_ts();
    table
        .entries
        .get_mut(&key)
        .expect("request gate entry")
        .last_seen_at = now - REQUEST_GATE_LOCK_TTL_SECS - 1;
    table.last_cleanup_at = now - REQUEST_GATE_LOCK_CLEANUP_INTERVAL_SECS - 1;
    drop(table);

    let second = request_gate_lock("gk_1", "/v1/responses", Some("gpt-5.3-codex"));
    assert!(Arc::ptr_eq(&first, &second));
}
