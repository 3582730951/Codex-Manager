use super::*;

#[test]
fn gateway_cpa_no_cookie_header_mode_suppresses_affinity_headers_on_responses() {
    let _lock = lock_env();
    let dir = new_test_dir("codexmanager-gateway-cpa-no-cookie-header-mode");
    let db_path: PathBuf = dir.join("codexmanager.db");

    let _db_guard = EnvGuard::set("CODEXMANAGER_DB_PATH", db_path.to_string_lossy().as_ref());
    let _mode_guard = EnvGuard::set("CODEXMANAGER_CPA_NO_COOKIE_HEADER_MODE", "1");
    let _cookie_guard = EnvGuard::set("CODEXMANAGER_UPSTREAM_COOKIE", "cf_clearance=still_present");

    let upstream_response = serde_json::json!({
        "id": "resp_cpa_mode",
        "model": "gpt-5.3-codex",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "ok" }]
        }],
        "usage": { "input_tokens": 3, "output_tokens": 2, "total_tokens": 5 }
    });
    let upstream_response =
        serde_json::to_string(&upstream_response).expect("serialize upstream response");
    let (upstream_addr, upstream_rx, upstream_join) = start_mock_upstream_once(&upstream_response);
    let upstream_base = format!("http://{upstream_addr}/backend-api/codex");
    let _upstream_guard = EnvGuard::set("CODEXMANAGER_UPSTREAM_BASE_URL", &upstream_base);

    let storage = Storage::open(&db_path).expect("open db");
    storage.init().expect("init db");
    let now = now_ts();

    storage
        .insert_account(&Account {
            id: "acc_cpa_no_cookie".to_string(),
            label: "cpa-mode".to_string(),
            issuer: "https://auth.openai.com".to_string(),
            chatgpt_account_id: Some("chatgpt_acc_cpa_mode".to_string()),
            workspace_id: None,
            group_name: None,
            sort: 1,
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        })
        .expect("insert account");
    storage
        .insert_token(&Token {
            account_id: "acc_cpa_no_cookie".to_string(),
            id_token: String::new(),
            access_token: "access_token_cpa_mode".to_string(),
            refresh_token: String::new(),
            api_key_access_token: Some("api_access_token_cpa_mode".to_string()),
            last_refresh: now,
        })
        .expect("insert token");

    let platform_key = "pk_cpa_no_cookie";
    storage
        .insert_api_key(&ApiKey {
            id: "gk_cpa_no_cookie".to_string(),
            name: Some("cpa-no-cookie".to_string()),
            model_slug: Some("gpt-5.3-codex".to_string()),
            reasoning_effort: None,
            client_type: "codex".to_string(),
            protocol_type: "openai_compat".to_string(),
            auth_scheme: "authorization_bearer".to_string(),
            upstream_base_url: None,
            static_headers_json: None,
            key_hash: hash_platform_key_for_test(platform_key),
            status: "active".to_string(),
            created_at: now,
            last_used_at: None,
        })
        .expect("insert api key");

    let server = codexmanager_service::start_one_shot_server().expect("start server");
    let req_body = r#"{"model":"gpt-5.3-codex","input":"hello","stream":false}"#;
    let (status, response_body) = post_http_raw(
        &server.addr,
        "/v1/responses",
        req_body,
        &[
            ("Content-Type", "application/json"),
            ("Authorization", &format!("Bearer {platform_key}")),
            ("x-codex-turn-state", "gAAA_dummy_turn_state_blob"),
            ("Conversation_id", "conv_dummy"),
            ("Session_id", "sess_dummy"),
        ],
    );
    server.join();
    assert_eq!(status, 200, "gateway response: {response_body}");

    let captured = upstream_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("receive upstream request");
    upstream_join.join().expect("join upstream");

    assert_eq!(captured.path, "/backend-api/codex/responses");
    assert!(!captured.headers.contains_key("x-codex-turn-state"));
    assert!(!captured.headers.contains_key("conversation_id"));
    assert!(!captured.headers.contains_key("openai-beta"));
    assert!(!captured.headers.contains_key("chatgpt-account-id"));
    assert_eq!(
        captured.headers.get("cookie").map(String::as_str),
        Some("cf_clearance=still_present")
    );
    assert_eq!(
        captured.headers.get("session_id").map(String::as_str),
        Some("sess_dummy")
    );
    let upstream_payload: serde_json::Value =
        serde_json::from_slice(&captured.body).expect("parse upstream payload");
    assert_eq!(upstream_payload["stream"], true);
    assert!(upstream_payload["input"].is_array());
    assert_eq!(upstream_payload["input"][0]["type"], "message");
    assert_eq!(upstream_payload["input"][0]["role"], "user");
    assert_eq!(
        upstream_payload["input"][0]["content"][0]["type"],
        "input_text"
    );
    assert_eq!(upstream_payload["input"][0]["content"][0]["text"], "hello");
}

#[test]
fn gateway_cpa_no_cookie_header_mode_binds_prompt_cache_key_to_session_only() {
    let _lock = lock_env();
    let dir = new_test_dir("codexmanager-gateway-cpa-no-cookie-prompt-cache");
    let db_path: PathBuf = dir.join("codexmanager.db");

    let _db_guard = EnvGuard::set("CODEXMANAGER_DB_PATH", db_path.to_string_lossy().as_ref());
    let _mode_guard = EnvGuard::set("CODEXMANAGER_CPA_NO_COOKIE_HEADER_MODE", "1");
    let _cookie_guard = EnvGuard::set("CODEXMANAGER_UPSTREAM_COOKIE", "");

    let upstream_response = serde_json::json!({
        "id": "resp_cpa_prompt_cache",
        "model": "gpt-5.3-codex",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "ok" }]
        }],
        "usage": { "input_tokens": 3, "output_tokens": 2, "total_tokens": 5 }
    });
    let upstream_response =
        serde_json::to_string(&upstream_response).expect("serialize upstream response");
    let (upstream_addr, upstream_rx, upstream_join) = start_mock_upstream_once(&upstream_response);
    let upstream_base = format!("http://{upstream_addr}/backend-api/codex");
    let _upstream_guard = EnvGuard::set("CODEXMANAGER_UPSTREAM_BASE_URL", &upstream_base);

    let storage = Storage::open(&db_path).expect("open db");
    storage.init().expect("init db");
    let now = now_ts();

    storage
        .insert_account(&Account {
            id: "acc_cpa_prompt_cache".to_string(),
            label: "cpa-prompt-cache".to_string(),
            issuer: "https://auth.openai.com".to_string(),
            chatgpt_account_id: Some("chatgpt_acc_cpa_prompt_cache".to_string()),
            workspace_id: None,
            group_name: None,
            sort: 1,
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        })
        .expect("insert account");
    storage
        .insert_token(&Token {
            account_id: "acc_cpa_prompt_cache".to_string(),
            id_token: String::new(),
            access_token: "access_token_cpa_prompt_cache".to_string(),
            refresh_token: String::new(),
            api_key_access_token: Some("api_access_token_cpa_prompt_cache".to_string()),
            last_refresh: now,
        })
        .expect("insert token");

    let platform_key = "pk_cpa_prompt_cache";
    storage
        .insert_api_key(&ApiKey {
            id: "gk_cpa_prompt_cache".to_string(),
            name: Some("cpa-prompt-cache".to_string()),
            model_slug: Some("gpt-5.3-codex".to_string()),
            reasoning_effort: None,
            client_type: "codex".to_string(),
            protocol_type: "openai_compat".to_string(),
            auth_scheme: "authorization_bearer".to_string(),
            upstream_base_url: None,
            static_headers_json: None,
            key_hash: hash_platform_key_for_test(platform_key),
            status: "active".to_string(),
            created_at: now,
            last_used_at: None,
        })
        .expect("insert api key");

    let server = codexmanager_service::start_one_shot_server().expect("start server");
    let req_body = r#"{"model":"gpt-5.3-codex","prompt_cache_key":"cache_anchor_123","input":"hello","stream":false}"#;
    let (status, response_body) = post_http_raw(
        &server.addr,
        "/v1/responses",
        req_body,
        &[
            ("Content-Type", "application/json"),
            ("Authorization", &format!("Bearer {platform_key}")),
            ("Session_id", "legacy_session_should_be_overridden"),
            (
                "Conversation_id",
                "legacy_conversation_should_be_overridden",
            ),
            ("x-codex-turn-state", "legacy_turn_state_should_be_dropped"),
        ],
    );
    server.join();
    assert_eq!(status, 200, "gateway response: {response_body}");

    let captured = upstream_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("receive upstream request");
    upstream_join.join().expect("join upstream");

    assert_eq!(
        captured.headers.get("session_id").map(String::as_str),
        Some("cache_anchor_123")
    );
    assert!(!captured.headers.contains_key("conversation_id"));
    assert!(!captured.headers.contains_key("x-codex-turn-state"));
    assert!(!captured.headers.contains_key("openai-beta"));
    assert!(!captured.headers.contains_key("chatgpt-account-id"));
}

#[test]
fn gateway_cpa_no_cookie_header_mode_skips_post_retries_on_404() {
    let _lock = lock_env();
    let dir = new_test_dir("codexmanager-gateway-cpa-no-cookie-no-post-retry");
    let db_path: PathBuf = dir.join("codexmanager.db");

    let _db_guard = EnvGuard::set("CODEXMANAGER_DB_PATH", db_path.to_string_lossy().as_ref());
    let _mode_guard = EnvGuard::set("CODEXMANAGER_CPA_NO_COOKIE_HEADER_MODE", "1");
    let _cookie_guard = EnvGuard::set("CODEXMANAGER_UPSTREAM_COOKIE", "");

    let err_body = serde_json::json!({
        "error": {
            "message": "not found for this account",
            "type": "invalid_request_error"
        }
    });
    let ok_body = serde_json::json!({
        "id": "resp_should_not_be_hit",
        "model": "gpt-5.3-codex",
        "output": [{
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": "unexpected" }]
        }],
        "usage": { "input_tokens": 3, "output_tokens": 2, "total_tokens": 5 }
    });
    let (upstream_addr, upstream_rx, upstream_join) = start_mock_upstream_sequence_lenient(
        vec![
            (
                404,
                serde_json::to_string(&err_body).expect("serialize 404 body"),
            ),
            (
                200,
                serde_json::to_string(&ok_body).expect("serialize 200 body"),
            ),
        ],
        Duration::from_millis(350),
    );
    let upstream_base = format!("http://{upstream_addr}/backend-api/codex");
    let _upstream_guard = EnvGuard::set("CODEXMANAGER_UPSTREAM_BASE_URL", &upstream_base);

    let storage = Storage::open(&db_path).expect("open db");
    storage.init().expect("init db");
    let now = now_ts();

    storage
        .insert_account(&Account {
            id: "acc_cpa_no_retry".to_string(),
            label: "cpa-no-retry".to_string(),
            issuer: "https://auth.openai.com".to_string(),
            chatgpt_account_id: Some("chatgpt_acc_cpa_no_retry".to_string()),
            workspace_id: None,
            group_name: None,
            sort: 1,
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        })
        .expect("insert account");
    storage
        .insert_token(&Token {
            account_id: "acc_cpa_no_retry".to_string(),
            id_token: String::new(),
            access_token: "access_token_cpa_no_retry".to_string(),
            refresh_token: String::new(),
            api_key_access_token: Some("api_access_token_cpa_no_retry".to_string()),
            last_refresh: now,
        })
        .expect("insert token");

    let platform_key = "pk_cpa_no_retry";
    storage
        .insert_api_key(&ApiKey {
            id: "gk_cpa_no_retry".to_string(),
            name: Some("cpa-no-retry".to_string()),
            model_slug: Some("gpt-5.3-codex".to_string()),
            reasoning_effort: None,
            client_type: "codex".to_string(),
            protocol_type: "openai_compat".to_string(),
            auth_scheme: "authorization_bearer".to_string(),
            upstream_base_url: None,
            static_headers_json: None,
            key_hash: hash_platform_key_for_test(platform_key),
            status: "active".to_string(),
            created_at: now,
            last_used_at: None,
        })
        .expect("insert api key");

    let server = codexmanager_service::start_one_shot_server().expect("start server");
    let req_body = r#"{"model":"gpt-5.3-codex","input":"hello","stream":false}"#;
    let (status, response_body) = post_http_raw(
        &server.addr,
        "/v1/responses",
        req_body,
        &[
            ("Content-Type", "application/json"),
            ("Authorization", &format!("Bearer {platform_key}")),
        ],
    );
    server.join();

    assert_eq!(
        status, 404,
        "cpa no-cookie mode should keep first 404 response without post retries, body: {response_body}"
    );

    let first = upstream_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("receive first upstream request");
    assert_eq!(first.path, "/backend-api/codex/responses");

    let second = upstream_rx.recv_timeout(Duration::from_millis(300));
    assert!(
        second.is_err(),
        "unexpected second upstream request in cpa no-cookie mode: {:?}",
        second.ok().map(|item| item.path)
    );

    upstream_join.join().expect("join upstream");
}
