use codexmanager_core::storage::{now_ts, Account, ApiKey, Storage, Token, UsageSnapshotRecord};
use sha2::{Digest, Sha256};

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn platform_key_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(format!("{byte:02x}").as_str());
    }
    out
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = env("AFFINITY_TEST_DB_PATH", "/data/codexmanager.db");
    let platform_key = env("AFFINITY_TEST_PLATFORM_KEY", "cm_affinity_test_key");
    let upstream_base = env("AFFINITY_TEST_UPSTREAM_BASE", "http://mock-upstream:18080");
    let now = now_ts();

    let storage = Storage::open(db_path)?;
    storage.init()?;

    let accounts = [
        ("aff-acc-1", "mock-account-1"),
        ("aff-acc-2", "mock-account-2"),
        ("aff-acc-3", "mock-account-3"),
        ("aff-acc-4", "mock-account-4"),
        ("aff-acc-5", "mock-account-5"),
    ];

    for (idx, (account_id, token_value)) in accounts.iter().enumerate() {
        storage.insert_account(&Account {
            id: (*account_id).to_string(),
            label: (*account_id).to_string(),
            issuer: "https://mock.auth.invalid".to_string(),
            chatgpt_account_id: Some(format!("chatgpt-{}", idx + 1)),
            workspace_id: Some(format!("workspace-{}", idx + 1)),
            group_name: None,
            sort: idx as i64,
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        })?;
        storage.insert_token(&Token {
            account_id: (*account_id).to_string(),
            id_token: format!("id-{token_value}"),
            access_token: (*token_value).to_string(),
            refresh_token: format!("refresh-{token_value}"),
            api_key_access_token: None,
            last_refresh: now,
        })?;
    }

    storage.insert_api_key(&ApiKey {
        id: "aff-key-1".to_string(),
        name: Some("affinity-mock".to_string()),
        model_slug: Some("gpt-5.4".to_string()),
        reasoning_effort: None,
        service_tier: None,
        rotation_strategy: "account_rotation".to_string(),
        aggregate_api_id: None,
        aggregate_api_url: None,
        client_type: "codex".to_string(),
        protocol_type: "openai_compat".to_string(),
        auth_scheme: "authorization_bearer".to_string(),
        upstream_base_url: Some(upstream_base),
        static_headers_json: None,
        key_hash: platform_key_hash(platform_key.as_str()),
        status: "active".to_string(),
        created_at: now,
        last_used_at: None,
    })?;

    for (idx, (account_id, _)) in accounts.iter().enumerate() {
        storage.insert_usage_snapshot(&UsageSnapshotRecord {
            account_id: (*account_id).to_string(),
            used_percent: Some(((idx + 1) * 10) as f64),
            window_minutes: Some(60),
            resets_at: Some(now + 3600),
            secondary_used_percent: None,
            secondary_window_minutes: None,
            secondary_resets_at: None,
            credits_json: None,
            captured_at: now,
        })?;
    }

    println!("AFFINITY_TEST_PLATFORM_KEY={platform_key}");
    println!(
        "AFFINITY_TEST_PLATFORM_KEY_HASH={}",
        platform_key_hash(platform_key.as_str())
    );
    Ok(())
}
