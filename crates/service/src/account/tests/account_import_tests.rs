use super::{
    extract_token_payload, import_account_auth_json, import_single_item,
    resolve_logical_account_id, ExistingAccountIndex, ImportTokenPayload,
};
use crate::account_identity::build_account_storage_id;
use crate::test_support;
use codexmanager_core::storage::{now_ts, Account, Storage};
use serde_json::json;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const TEST_ID_TOKEN_WS_A: &str = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJzdWItMSIsImVtYWlsIjoidGVzdEBleGFtcGxlLmNvbSIsIndvcmtzcGFjZV9pZCI6IndzLWEiLCJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiY2dwdC0xIn19.sig";
const TEST_ID_TOKEN_META: &str = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJzdWItMSIsImVtYWlsIjoibWV0YUBleGFtcGxlLmNvbSIsIndvcmtzcGFjZV9pZCI6IndzLW1ldGEiLCJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiY2dwdC1tZXRhIn19.sig";

fn unique_temp_db_path() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("codexmanager-account-import-test-{unique}.db"))
}

fn payload() -> ImportTokenPayload {
    ImportTokenPayload {
        access_token: "access".to_string(),
        id_token: "id".to_string(),
        refresh_token: "refresh".to_string(),
        account_id_hint: None,
        chatgpt_account_id_hint: None,
    }
}

#[test]
fn resolve_logical_account_id_distinguishes_workspace_under_same_chatgpt() {
    let input = payload();
    let a = resolve_logical_account_id(
        &input,
        Some("sub-1"),
        Some("cgpt-1"),
        Some("ws-a"),
        Some("same-fp"),
    )
    .expect("resolve ws-a");
    let b = resolve_logical_account_id(
        &input,
        Some("sub-1"),
        Some("cgpt-1"),
        Some("ws-b"),
        Some("same-fp"),
    )
    .expect("resolve ws-b");

    assert_ne!(a, b);
}

#[test]
fn resolve_logical_account_id_is_stable_when_scope_is_stable() {
    let input = payload();
    let first = resolve_logical_account_id(
        &input,
        Some("sub-1"),
        Some("cgpt-1"),
        Some("ws-a"),
        Some("fp-1"),
    )
    .expect("resolve first");
    let second = resolve_logical_account_id(
        &input,
        Some("sub-1"),
        Some("cgpt-1"),
        Some("ws-a"),
        Some("fp-2"),
    )
    .expect("resolve second");

    assert_eq!(first, second);
    assert_eq!(
        first,
        build_account_storage_id("sub-1", Some("cgpt-1"), Some("ws-a"), None)
    );
}

#[test]
fn existing_account_index_next_sort_uses_step_five() {
    let storage = Storage::open_in_memory().expect("open in memory");
    storage.init().expect("init");
    let now = now_ts();
    storage
        .insert_account(&Account {
            id: "acc-1".to_string(),
            label: "acc-1".to_string(),
            issuer: "https://auth.openai.com".to_string(),
            chatgpt_account_id: Some("cgpt-1".to_string()),
            workspace_id: Some("ws-1".to_string()),
            group_name: None,
            sort: 0,
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        })
        .expect("insert acc-1");
    storage
        .insert_account(&Account {
            id: "acc-2".to_string(),
            label: "acc-2".to_string(),
            issuer: "https://auth.openai.com".to_string(),
            chatgpt_account_id: Some("cgpt-2".to_string()),
            workspace_id: Some("ws-2".to_string()),
            group_name: None,
            sort: 9,
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        })
        .expect("insert acc-2");

    let idx = ExistingAccountIndex::build(&storage).expect("build index");
    assert_eq!(idx.next_sort, 14);
}

#[test]
fn extract_token_payload_supports_flat_codex_format() {
    let value = json!({
        "type": "codex",
        "email": "u@example.com",
        "id_token": "id.flat",
        "account_id": "acc-flat",
        "access_token": "access.flat",
        "refresh_token": "refresh.flat"
    });

    let payload = extract_token_payload(&value).expect("parse flat payload");
    assert_eq!(payload.access_token, "access.flat");
    assert_eq!(payload.id_token, "id.flat");
    assert_eq!(payload.refresh_token, "refresh.flat");
    assert_eq!(payload.account_id_hint.as_deref(), Some("acc-flat"));
    assert_eq!(payload.chatgpt_account_id_hint, None);
}

#[test]
fn extract_token_payload_supports_camel_case_fields() {
    let value = json!({
        "tokens": {
            "idToken": "id.camel",
            "accessToken": "access.camel",
            "refreshToken": "refresh.camel",
            "accountId": "acc-camel",
            "chatgptAccountId": "cgpt-camel"
        }
    });

    let payload = extract_token_payload(&value).expect("parse camel payload");
    assert_eq!(payload.access_token, "access.camel");
    assert_eq!(payload.id_token, "id.camel");
    assert_eq!(payload.refresh_token, "refresh.camel");
    assert_eq!(payload.account_id_hint.as_deref(), Some("acc-camel"));
    assert_eq!(
        payload.chatgpt_account_id_hint.as_deref(),
        Some("cgpt-camel")
    );
}

#[test]
fn import_single_item_reuses_existing_login_account_by_scope_identity() {
    let storage = Storage::open_in_memory().expect("open in memory");
    storage.init().expect("init");
    let now = now_ts();
    let existing_id = build_account_storage_id("sub-1", Some("cgpt-1"), Some("ws-a"), None);
    storage
        .insert_account(&Account {
            id: existing_id.clone(),
            label: "existing".to_string(),
            issuer: "https://auth.openai.com".to_string(),
            chatgpt_account_id: Some("cgpt-1".to_string()),
            workspace_id: Some("ws-a".to_string()),
            group_name: Some("LOGIN".to_string()),
            sort: 0,
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        })
        .expect("insert existing account");

    let mut idx = ExistingAccountIndex::build(&storage).expect("build index");
    let item = json!({
        "tokens": {
            "access_token": "access.import",
            "id_token": TEST_ID_TOKEN_WS_A,
            "refresh_token": "refresh.import",
            "account_id": "legacy-import-id"
        }
    });

    let created = import_single_item(&storage, &mut idx, &item, 1).expect("import item");
    assert!(!created);

    let accounts = storage.list_accounts().expect("list accounts");
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].id, existing_id);
    assert_eq!(accounts[0].group_name.as_deref(), Some("LOGIN"));
    assert!(storage
        .find_account_metadata(&accounts[0].id)
        .expect("find metadata")
        .is_none());

    let token = storage
        .find_token_by_account_id(&accounts[0].id)
        .expect("find token")
        .expect("token");
    assert_eq!(token.account_id, accounts[0].id);
}

#[test]
fn import_single_item_prefers_meta_fields_for_new_account() {
    let storage = Storage::open_in_memory().expect("open in memory");
    storage.init().expect("init");
    let mut idx = ExistingAccountIndex::build(&storage).expect("build index");
    let item = json!({
        "tokens": {
            "access_token": "access.meta",
            "id_token": TEST_ID_TOKEN_META,
            "refresh_token": "refresh.meta",
            "account_id": "exported-account-id"
        },
        "meta": {
            "label": "Meta Label",
            "issuer": "https://issuer.example",
            "note": "Meta Note",
            "tags": ["高频", "团队A"],
            "workspace_id": "ws-manual",
            "chatgpt_account_id": "cgpt-manual"
        }
    });

    let created = import_single_item(&storage, &mut idx, &item, 1).expect("import item");
    assert!(created);

    let accounts = storage.list_accounts().expect("list accounts");
    assert_eq!(accounts.len(), 1);
    assert_eq!(
        accounts[0].id,
        build_account_storage_id("sub-1", Some("cgpt-manual"), Some("ws-manual"), None)
    );
    assert_eq!(accounts[0].label, "Meta Label");
    assert_eq!(accounts[0].issuer, "https://issuer.example");
    assert_eq!(accounts[0].group_name.as_deref(), Some("IMPORT"));
    assert_eq!(
        accounts[0].chatgpt_account_id.as_deref(),
        Some("cgpt-manual")
    );
    assert_eq!(accounts[0].workspace_id.as_deref(), Some("ws-manual"));
    let metadata = storage
        .find_account_metadata(&accounts[0].id)
        .expect("find metadata")
        .expect("metadata");
    assert_eq!(metadata.note.as_deref(), Some("Meta Note"));
    assert_eq!(metadata.tags.as_deref(), Some("高频,团队A"));
}

#[test]
fn import_account_auth_json_keeps_valid_items_when_one_content_is_invalid() {
    let _guard = test_support::env_lock().lock().expect("env lock");
    let db_path = unique_temp_db_path();
    let previous_db_path = std::env::var("CODEXMANAGER_DB_PATH").ok();
    std::env::set_var("CODEXMANAGER_DB_PATH", &db_path);

    let storage = Storage::open(&db_path).expect("open storage");
    storage.init().expect("init storage");
    drop(storage);

    let result = import_account_auth_json(vec![
        json!({
            "type": "codex",
            "email": "valid@example.com",
            "id_token": TEST_ID_TOKEN_META,
            "account_id": "valid-account",
            "access_token": "access.valid",
            "refresh_token": "refresh.valid"
        })
        .to_string(),
        "not-json".to_string(),
    ])
    .expect("import account auth json");

    assert_eq!(result.total, 2);
    assert_eq!(result.created, 1);
    assert_eq!(result.updated, 0);
    assert_eq!(result.failed, 1);
    assert!(result
        .errors
        .iter()
        .any(|item| { item.message.contains("invalid JSON object stream") }));

    let storage = Storage::open(&db_path).expect("reopen storage");
    let accounts = storage.list_accounts().expect("list accounts");
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].label, "meta@example.com");

    if let Some(value) = previous_db_path {
        std::env::set_var("CODEXMANAGER_DB_PATH", value);
    } else {
        std::env::remove_var("CODEXMANAGER_DB_PATH");
    }
    let _ = std::fs::remove_file(&db_path);
}
