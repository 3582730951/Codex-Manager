use base64::Engine;
use codexmanager_core::storage::{now_ts, ApiKey, Storage};
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

static OAUTH_TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

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
    }
}

fn hash_platform_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn new_test_dir(prefix: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("{prefix}-{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    dir
}

#[test]
fn oauth_cli_authorize_returns_child_key_and_gateway_uses_owner_key_for_logs() {
    let _env_guard = OAUTH_TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = new_test_dir("codexmanager-oauth-cli");
    let db_path = dir.join("codexmanager.db");
    let _db_guard = EnvGuard::set("CODEXMANAGER_DB_PATH", db_path.to_string_lossy().as_ref());

    let storage = Storage::open(&db_path).expect("open db");
    storage.init().expect("init schema");
    storage
        .insert_api_key(&ApiKey {
            id: "pk_parent".to_string(),
            name: Some("Employee Parent".to_string()),
            model_slug: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: None,
            rotation_strategy: "account_rotation".to_string(),
            aggregate_api_id: None,
            aggregate_api_url: None,
            client_type: "codex".to_string(),
            protocol_type: "openai_compat".to_string(),
            auth_scheme: "authorization_bearer".to_string(),
            upstream_base_url: None,
            static_headers_json: None,
            key_hash: hash_platform_key("parent-secret"),
            status: "active".to_string(),
            created_at: now_ts(),
            last_used_at: None,
        })
        .expect("insert parent key");
    storage
        .upsert_api_key_secret("pk_parent", "parent-secret")
        .expect("save parent secret");
    storage
        .upsert_model_options_cache(
            "default",
            r#"[{"slug":"gpt-5.4","display_name":"GPT-5.4"}]"#,
            now_ts(),
        )
        .expect("seed model cache");

    let server = codexmanager_service::start_test_server().expect("start server");
    let base_url = format!("http://{}", server.addr);
    let client = Client::builder()
        .redirect(Policy::none())
        .build()
        .expect("build http client");

    let verifier = "oauth-cli-verifier";
    let authorize_response = client
        .post(format!("{base_url}/oauth/authorize/approve"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "response_type=code&client_id=cli-client&redirect_uri={}&state=test-state&code_challenge={}&code_challenge_method=S256&employee_api_key={}",
            urlencoding::encode("http://127.0.0.1:1455/callback"),
            urlencoding::encode(code_challenge(verifier).as_str()),
            urlencoding::encode("parent-secret"),
        ))
        .send()
        .expect("authorize approve");
    assert_eq!(authorize_response.status(), reqwest::StatusCode::FOUND);
    let redirect = authorize_response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("redirect location");
    let redirect_url = url::Url::parse(redirect).expect("parse redirect url");
    let code = redirect_url
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())
        .expect("authorization code");

    let token_response: serde_json::Value = client
        .post(format!("{base_url}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id=cli-client&code_verifier={}",
            urlencoding::encode(code.as_str()),
            urlencoding::encode("http://127.0.0.1:1455/callback"),
            urlencoding::encode(verifier),
        ))
        .send()
        .expect("token exchange")
        .json()
        .expect("parse token response");

    let oauth_access_token = token_response
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .expect("oauth access token");
    let id_token = token_response
        .get("id_token")
        .and_then(serde_json::Value::as_str)
        .expect("id token");
    let refresh_token = token_response
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .expect("refresh token");
    assert_ne!(oauth_access_token, "parent-secret");
    assert!(oauth_access_token.contains('.'));

    let api_key_exchange_response: serde_json::Value = client
        .post(format!("{base_url}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange&client_id=cli-client&requested_token=openai-api-key&subject_token_type=urn%3Aietf%3Aparams%3Aoauth%3Atoken-type%3Aid_token&subject_token={}",
            urlencoding::encode(id_token),
        ))
        .send()
        .expect("api key exchange")
        .json()
        .expect("parse api key exchange");
    let child_access_token = api_key_exchange_response
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .expect("child access token");
    assert_ne!(child_access_token, oauth_access_token);
    assert!(!child_access_token.contains('.'));

    let models_response = client
        .get(format!("{base_url}/v1/models"))
        .bearer_auth(child_access_token)
        .send()
        .expect("models request");
    assert_eq!(models_response.status(), reqwest::StatusCode::OK);

    let refreshed_response: serde_json::Value = client
        .post(format!("{base_url}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=refresh_token&client_id=cli-client&refresh_token={}",
            urlencoding::encode(refresh_token),
        ))
        .send()
        .expect("refresh token exchange")
        .json()
        .expect("parse refresh response");
    let refreshed_access_token = refreshed_response
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .expect("refreshed oauth access token");
    assert_ne!(refreshed_access_token, child_access_token);
    assert!(refreshed_access_token.contains('.'));

    let storage = Storage::open(&db_path).expect("re-open db");
    let child_keys = storage
        .list_cli_child_keys_for_owner("pk_parent")
        .expect("list child keys");
    assert_eq!(child_keys.len(), 1);
    let owner_context = storage
        .lookup_api_key_owner_context(child_keys[0].child_key_id.as_str())
        .expect("lookup owner context")
        .expect("owner context exists");
    assert_eq!(owner_context.owner_key_id, "pk_parent");
    assert!(!owner_context.cli_instance_uuid.trim().is_empty());

    let request_logs = storage
        .list_request_logs(None, 10)
        .expect("list request logs");
    let models_log = request_logs
        .iter()
        .find(|log| log.request_path == "/v1/models")
        .expect("models request log");
    assert_eq!(models_log.owner_key_id.as_deref(), Some("pk_parent"));
    assert_eq!(
        models_log.key_id.as_deref(),
        Some(child_keys[0].child_key_id.as_str())
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn oauth_cli_authorize_rejects_non_loopback_redirect_uri() {
    let _env_guard = OAUTH_TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = new_test_dir("codexmanager-oauth-cli-redirect");
    let db_path = dir.join("codexmanager.db");
    let _db_guard = EnvGuard::set("CODEXMANAGER_DB_PATH", db_path.to_string_lossy().as_ref());

    let storage = Storage::open(&db_path).expect("open db");
    storage.init().expect("init schema");
    storage
        .insert_api_key(&ApiKey {
            id: "pk_parent".to_string(),
            name: Some("Employee Parent".to_string()),
            model_slug: Some("gpt-5.4".to_string()),
            reasoning_effort: Some("medium".to_string()),
            service_tier: None,
            rotation_strategy: "account_rotation".to_string(),
            aggregate_api_id: None,
            aggregate_api_url: None,
            client_type: "codex".to_string(),
            protocol_type: "openai_compat".to_string(),
            auth_scheme: "authorization_bearer".to_string(),
            upstream_base_url: None,
            static_headers_json: None,
            key_hash: hash_platform_key("parent-secret"),
            status: "active".to_string(),
            created_at: now_ts(),
            last_used_at: None,
        })
        .expect("insert parent key");
    storage
        .upsert_api_key_secret("pk_parent", "parent-secret")
        .expect("save parent secret");

    let server = codexmanager_service::start_test_server().expect("start server");
    let base_url = format!("http://{}", server.addr);
    let client = Client::builder()
        .redirect(Policy::none())
        .build()
        .expect("build http client");

    let response = client
        .post(format!("{base_url}/oauth/authorize/approve"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "response_type=code&client_id=cli-client&redirect_uri={}&state=test-state&code_challenge={}&code_challenge_method=S256&employee_api_key={}",
            urlencoding::encode("https://example.com/callback"),
            urlencoding::encode(code_challenge("oauth-cli-verifier").as_str()),
            urlencoding::encode("parent-secret"),
        ))
        .send()
        .expect("authorize approve");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert!(response
        .text()
        .expect("read response body")
        .contains("loopback"));

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn oauth_cli_child_key_uses_latest_parent_model_config() {
    let _env_guard = OAUTH_TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = new_test_dir("codexmanager-oauth-cli-parent-config");
    let db_path = dir.join("codexmanager.db");
    let _db_guard = EnvGuard::set("CODEXMANAGER_DB_PATH", db_path.to_string_lossy().as_ref());

    let storage = Storage::open(&db_path).expect("open db");
    storage.init().expect("init schema");
    storage
        .insert_api_key(&ApiKey {
            id: "pk_parent".to_string(),
            name: Some("Employee Parent".to_string()),
            model_slug: Some("gpt-4.1".to_string()),
            reasoning_effort: Some("low".to_string()),
            service_tier: None,
            rotation_strategy: "account_rotation".to_string(),
            aggregate_api_id: None,
            aggregate_api_url: None,
            client_type: "codex".to_string(),
            protocol_type: "openai_compat".to_string(),
            auth_scheme: "authorization_bearer".to_string(),
            upstream_base_url: None,
            static_headers_json: None,
            key_hash: hash_platform_key("parent-secret"),
            status: "active".to_string(),
            created_at: now_ts(),
            last_used_at: None,
        })
        .expect("insert parent key");
    storage
        .upsert_api_key_secret("pk_parent", "parent-secret")
        .expect("save parent secret");

    let server = codexmanager_service::start_test_server().expect("start server");
    let base_url = format!("http://{}", server.addr);
    let client = Client::builder()
        .redirect(Policy::none())
        .build()
        .expect("build http client");

    let verifier = "oauth-cli-verifier";
    let authorize_response = client
        .post(format!("{base_url}/oauth/authorize/approve"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "response_type=code&client_id=cli-client&redirect_uri={}&state=test-state&code_challenge={}&code_challenge_method=S256&employee_api_key={}",
            urlencoding::encode("http://127.0.0.1:1455/callback"),
            urlencoding::encode(code_challenge(verifier).as_str()),
            urlencoding::encode("parent-secret"),
        ))
        .send()
        .expect("authorize approve");
    assert_eq!(authorize_response.status(), reqwest::StatusCode::FOUND);
    let redirect = authorize_response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("redirect location");
    let redirect_url = url::Url::parse(redirect).expect("parse redirect url");
    let code = redirect_url
        .query_pairs()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.into_owned())
        .expect("authorization code");

    let token_response: serde_json::Value = client
        .post(format!("{base_url}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=authorization_code&code={}&redirect_uri={}&client_id=cli-client&code_verifier={}",
            urlencoding::encode(code.as_str()),
            urlencoding::encode("http://127.0.0.1:1455/callback"),
            urlencoding::encode(verifier),
        ))
        .send()
        .expect("token exchange")
        .json()
        .expect("parse token response");
    let id_token = token_response
        .get("id_token")
        .and_then(serde_json::Value::as_str)
        .expect("id token");

    let api_key_exchange_response: serde_json::Value = client
        .post(format!("{base_url}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(format!(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange&client_id=cli-client&requested_token=openai-api-key&subject_token_type=urn%3Aietf%3Aparams%3Aoauth%3Atoken-type%3Aid_token&subject_token={}",
            urlencoding::encode(id_token),
        ))
        .send()
        .expect("api key exchange")
        .json()
        .expect("parse api key exchange");
    let child_access_token = api_key_exchange_response
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .expect("child access token");

    storage
        .update_api_key_model_config("pk_parent", Some("gpt-5.4"), Some("high"), None)
        .expect("update parent model config");

    let models_response: serde_json::Value = client
        .get(format!("{base_url}/v1/models"))
        .bearer_auth(child_access_token)
        .send()
        .expect("models request")
        .json()
        .expect("parse models response");
    let model_ids = models_response
        .get("data")
        .and_then(serde_json::Value::as_array)
        .expect("models array")
        .iter()
        .filter_map(|item| item.get("id").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();

    assert!(model_ids.contains(&"gpt-5.4"));
    assert!(!model_ids.contains(&"gpt-4.1"));

    let _ = fs::remove_dir_all(dir);
}
