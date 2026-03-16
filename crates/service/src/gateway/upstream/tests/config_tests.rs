use reqwest::header::HeaderValue;
use std::sync::Mutex;

use super::{
    reload_from_env, resolve_upstream_fallback_base_url, should_try_openai_fallback,
    should_try_openai_fallback_by_status,
};

static TEST_MUTEX: Mutex<()> = Mutex::new(());

#[test]
fn fallback_status_trigger_is_limited_to_responses_path() {
    assert!(should_try_openai_fallback_by_status(
        "https://chatgpt.com/backend-api/codex",
        "/v1/responses",
        429
    ));
    assert!(!should_try_openai_fallback_by_status(
        "https://chatgpt.com/backend-api/codex",
        "/v1/chat/completions",
        429
    ));
}

#[test]
fn fallback_content_type_trigger_is_limited_to_responses_path() {
    let html = HeaderValue::from_static("text/html; charset=utf-8");
    assert!(should_try_openai_fallback(
        "https://chatgpt.com/backend-api/codex",
        "/v1/responses",
        Some(&html)
    ));
    assert!(should_try_openai_fallback(
        "https://chatgpt.com/backend-api/codex",
        "/v1/chat/completions",
        Some(&html)
    ));
}

#[test]
fn fallback_content_type_trigger_accepts_custom_codex_backend_base() {
    let html = HeaderValue::from_static("text/html; charset=utf-8");
    assert!(should_try_openai_fallback(
        "http://127.0.0.1:9000/backend-api/codex",
        "/v1/responses",
        Some(&html)
    ));
}

#[test]
fn fallback_base_defaults_to_disabled_until_env_is_set() {
    let _lock = TEST_MUTEX.lock().expect("lock env test");
    std::env::remove_var("CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL");
    reload_from_env();
    assert_eq!(
        resolve_upstream_fallback_base_url("https://chatgpt.com/backend-api/codex").as_deref(),
        None
    );
}

#[test]
fn fallback_base_honors_explicit_env_override() {
    let _lock = TEST_MUTEX.lock().expect("lock env test");
    std::env::set_var(
        "CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL",
        "https://api.openai.com/v1",
    );
    reload_from_env();
    assert_eq!(
        resolve_upstream_fallback_base_url("https://chatgpt.com/backend-api/codex").as_deref(),
        Some("https://api.openai.com/v1")
    );

    std::env::remove_var("CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL");
    reload_from_env();
}

#[test]
fn fallback_base_ignores_same_target_and_openai_primary() {
    let _lock = TEST_MUTEX.lock().expect("lock env test");
    std::env::set_var(
        "CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL",
        "https://chatgpt.com",
    );
    reload_from_env();
    assert_eq!(
        resolve_upstream_fallback_base_url("https://chatgpt.com/backend-api/codex").as_deref(),
        None
    );
    assert_eq!(
        resolve_upstream_fallback_base_url("https://api.openai.com/v1").as_deref(),
        None
    );

    std::env::remove_var("CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL");
    reload_from_env();
}
