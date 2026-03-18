use super::*;
use codexmanager_core::storage::{now_ts, ApiKey, Storage};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static APIKEY_MODELS_TEST_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.previous {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn unique_db_path(prefix: &str) -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir()
        .join(format!("{prefix}-{nonce}.db"))
        .to_string_lossy()
        .to_string()
}

fn insert_api_key(storage: &Storage, id: &str, model_slug: Option<&str>, protocol_type: &str) {
    storage
        .insert_api_key(&ApiKey {
            id: id.to_string(),
            name: Some(id.to_string()),
            model_slug: model_slug.map(|value| value.to_string()),
            reasoning_effort: None,
            client_type: "codex".to_string(),
            protocol_type: protocol_type.to_string(),
            auth_scheme: "authorization_bearer".to_string(),
            upstream_base_url: None,
            static_headers_json: None,
            key_hash: format!("hash_{id}"),
            status: "active".to_string(),
            created_at: now_ts(),
            last_used_at: None,
        })
        .expect("insert api key");
}

#[test]
fn read_model_options_without_cache_uses_local_fallback_models() {
    let _guard = APIKEY_MODELS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let db_path = unique_db_path("codexmanager-apikey-models-fallback");
    let _db_guard = EnvGuard::set("CODEXMANAGER_DB_PATH", &db_path);
    crate::gateway::reload_runtime_config_from_env();
    crate::storage_helpers::initialize_storage().expect("initialize storage");
    let storage = Storage::open(&db_path).expect("open storage");
    insert_api_key(
        &storage,
        "gk_apikey_models_fallback",
        Some("claude-sonnet-4"),
        "anthropic_native",
    );
    drop(storage);

    let result = read_model_options(false).expect("read model options");
    let slugs = result
        .items
        .iter()
        .map(|item| item.slug.as_str())
        .collect::<Vec<_>>();
    assert!(slugs.contains(&"gpt-5"));
    assert!(slugs.contains(&"claude-sonnet-4"));

    let _ = std::fs::remove_file(db_path);
}

#[test]
fn refresh_model_options_without_gateway_candidates_falls_back_and_caches() {
    let _guard = APIKEY_MODELS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let db_path = unique_db_path("codexmanager-apikey-models-refresh-fallback");
    let _db_guard = EnvGuard::set("CODEXMANAGER_DB_PATH", &db_path);
    crate::gateway::reload_runtime_config_from_env();
    crate::storage_helpers::initialize_storage().expect("initialize storage");
    let storage = Storage::open(&db_path).expect("open storage");
    insert_api_key(
        &storage,
        "gk_apikey_models_refresh_fallback",
        Some("claude-sonnet-4"),
        "anthropic_native",
    );
    drop(storage);

    let refreshed = read_model_options(true).expect("refresh model options");
    let refreshed_slugs = refreshed
        .items
        .iter()
        .map(|item| item.slug.as_str())
        .collect::<Vec<_>>();
    assert!(refreshed_slugs.contains(&"gpt-5"));
    assert!(refreshed_slugs.contains(&"claude-sonnet-4"));

    let cached = read_model_options(false).expect("read cached model options");
    let cached_slugs = cached
        .items
        .iter()
        .map(|item| item.slug.as_str())
        .collect::<Vec<_>>();
    assert!(cached_slugs.contains(&"gpt-5"));
    assert!(cached_slugs.contains(&"claude-sonnet-4"));

    let _ = std::fs::remove_file(db_path);
}
