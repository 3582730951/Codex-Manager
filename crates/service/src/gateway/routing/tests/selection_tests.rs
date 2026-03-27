use super::{clear_candidate_cache_for_tests, collect_gateway_candidates, CANDIDATE_CACHE_TTL_ENV};
use crate::test_support;
use codexmanager_core::storage::{now_ts, Account, Storage, Token, UsageSnapshotRecord};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_cache_db_path(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("codexmanager-{label}-{unique}.db"))
}

#[test]
fn candidate_snapshot_cache_reuses_recent_snapshot() {
    let _guard = test_support::env_lock().lock().expect("lock");
    let previous_ttl = std::env::var(CANDIDATE_CACHE_TTL_ENV).ok();
    let previous_db_path = std::env::var("CODEXMANAGER_DB_PATH").ok();
    let db_path = unique_cache_db_path("selection-cache-test-1");
    std::env::set_var(CANDIDATE_CACHE_TTL_ENV, "2000");
    std::env::set_var("CODEXMANAGER_DB_PATH", &db_path);
    super::reload_from_env();
    clear_candidate_cache_for_tests();

    let storage = Storage::open_in_memory().expect("open");
    storage.init().expect("init");
    storage
        .insert_account(&Account {
            id: "acc-cache-1".to_string(),
            label: "cached".to_string(),
            issuer: "issuer".to_string(),
            chatgpt_account_id: None,
            workspace_id: None,
            group_name: None,
            sort: 0,
            status: "active".to_string(),
            created_at: now_ts(),
            updated_at: now_ts(),
        })
        .expect("insert account");
    storage
        .insert_token(&Token {
            account_id: "acc-cache-1".to_string(),
            id_token: "id".to_string(),
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            api_key_access_token: None,
            last_refresh: now_ts(),
        })
        .expect("insert token");
    storage
        .insert_usage_snapshot(&UsageSnapshotRecord {
            account_id: "acc-cache-1".to_string(),
            used_percent: Some(10.0),
            window_minutes: Some(300),
            resets_at: None,
            secondary_used_percent: None,
            secondary_window_minutes: None,
            secondary_resets_at: None,
            credits_json: None,
            captured_at: now_ts(),
        })
        .expect("insert snapshot");

    let first = collect_gateway_candidates(&storage).expect("first candidates");
    assert_eq!(first.len(), 1);

    storage
        .update_account_status("acc-cache-1", "inactive")
        .expect("mark inactive");
    let second = collect_gateway_candidates(&storage).expect("second candidates");
    assert_eq!(second.len(), 1);

    clear_candidate_cache_for_tests();
    if let Some(value) = previous_ttl {
        std::env::set_var(CANDIDATE_CACHE_TTL_ENV, value);
    } else {
        std::env::remove_var(CANDIDATE_CACHE_TTL_ENV);
    }
    if let Some(value) = previous_db_path {
        std::env::set_var("CODEXMANAGER_DB_PATH", value);
    } else {
        std::env::remove_var("CODEXMANAGER_DB_PATH");
    }
    super::reload_from_env();
    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn candidates_follow_account_sort_order() {
    let _guard = test_support::env_lock().lock().expect("lock");
    let previous_ttl = std::env::var(CANDIDATE_CACHE_TTL_ENV).ok();
    let previous_db_path = std::env::var("CODEXMANAGER_DB_PATH").ok();
    let db_path = unique_cache_db_path("selection-cache-test-2");
    std::env::set_var(CANDIDATE_CACHE_TTL_ENV, "0");
    std::env::set_var("CODEXMANAGER_DB_PATH", &db_path);
    super::reload_from_env();
    clear_candidate_cache_for_tests();

    let storage = Storage::open_in_memory().expect("open");
    storage.init().expect("init");

    let now = now_ts();
    let accounts = vec![
        ("acc-sort-10", 10_i64),
        ("acc-sort-0", 0_i64),
        ("acc-sort-1", 1_i64),
    ];
    for (id, sort) in &accounts {
        storage
            .insert_account(&Account {
                id: (*id).to_string(),
                label: (*id).to_string(),
                issuer: "issuer".to_string(),
                chatgpt_account_id: None,
                workspace_id: None,
                group_name: None,
                sort: *sort,
                status: "active".to_string(),
                created_at: now,
                updated_at: now,
            })
            .expect("insert account");
        storage
            .insert_token(&Token {
                account_id: (*id).to_string(),
                id_token: "id".to_string(),
                access_token: "access".to_string(),
                refresh_token: "refresh".to_string(),
                api_key_access_token: None,
                last_refresh: now,
            })
            .expect("insert token");
        storage
            .insert_usage_snapshot(&UsageSnapshotRecord {
                account_id: (*id).to_string(),
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

    let candidates = collect_gateway_candidates(&storage).expect("collect candidates");
    let ordered_ids = candidates
        .iter()
        .map(|(account, _)| account.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ordered_ids, vec!["acc-sort-0", "acc-sort-1", "acc-sort-10"]);

    clear_candidate_cache_for_tests();
    if let Some(value) = previous_ttl {
        std::env::set_var(CANDIDATE_CACHE_TTL_ENV, value);
    } else {
        std::env::remove_var(CANDIDATE_CACHE_TTL_ENV);
    }
    if let Some(value) = previous_db_path {
        std::env::set_var("CODEXMANAGER_DB_PATH", value);
    } else {
        std::env::remove_var("CODEXMANAGER_DB_PATH");
    }
    super::reload_from_env();
    let _ = std::fs::remove_file(&db_path);
}
