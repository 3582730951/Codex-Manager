use reqwest::header::HeaderValue;

use super::{
    reload_from_env, resolve_upstream_fallback_base_url, should_try_openai_fallback,
    should_try_openai_fallback_by_status,
};

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
fn fallback_base_is_disabled_even_when_env_is_set() {
    std::env::remove_var("CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL");
    reload_from_env();
    assert_eq!(
        resolve_upstream_fallback_base_url("https://chatgpt.com/backend-api/codex").as_deref(),
        None
    );

    std::env::set_var(
        "CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL",
        "https://api.openai.com/v1",
    );
    reload_from_env();
    assert_eq!(
        resolve_upstream_fallback_base_url("https://chatgpt.com/backend-api/codex").as_deref(),
        None
    );

    std::env::remove_var("CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL");
    reload_from_env();
}
