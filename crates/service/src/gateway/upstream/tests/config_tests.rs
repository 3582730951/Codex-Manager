use reqwest::header::HeaderValue;

use super::{
    is_upstream_fallback_explicitly_configured, reload_from_env, resolve_upstream_fallback_base_url,
    should_try_openai_fallback, should_try_openai_fallback_by_status,
};

#[test]
fn fallback_status_trigger_requires_explicit_responses_fallback() {
    std::env::remove_var("CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL");
    reload_from_env();
    assert!(!should_try_openai_fallback_by_status(
        "https://chatgpt.com/backend-api/codex",
        "/v1/responses",
        429
    ));

    std::env::set_var(
        "CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL",
        "https://api.openai.com/v1",
    );
    reload_from_env();
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
fn fallback_content_type_trigger_requires_explicit_responses_fallback() {
    let html = HeaderValue::from_static("text/html; charset=utf-8");
    std::env::remove_var("CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL");
    reload_from_env();
    assert!(!should_try_openai_fallback(
        "https://chatgpt.com/backend-api/codex",
        "/v1/responses",
        Some(&html)
    ));

    std::env::set_var(
        "CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL",
        "https://api.openai.com/v1",
    );
    reload_from_env();
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
fn fallback_base_defaults_for_chatgpt_primary_without_env() {
    std::env::remove_var("CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL");
    reload_from_env();
    assert!(!is_upstream_fallback_explicitly_configured());
    assert_eq!(
        resolve_upstream_fallback_base_url("https://chatgpt.com/backend-api/codex").as_deref(),
        Some("https://api.openai.com/v1")
    );
}

#[test]
fn fallback_base_reads_env_for_chatgpt_primary() {
    std::env::set_var(
        "CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL",
        "https://api.openai.com/v1",
    );
    reload_from_env();
    assert!(is_upstream_fallback_explicitly_configured());
    assert_eq!(
        resolve_upstream_fallback_base_url("https://chatgpt.com/backend-api/codex").as_deref(),
        Some("https://api.openai.com/v1")
    );
}

#[test]
fn fallback_base_is_suppressed_for_openai_primary() {
    std::env::set_var(
        "CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL",
        "https://api.openai.com/v1",
    );
    reload_from_env();
    assert!(is_upstream_fallback_explicitly_configured());
    assert_eq!(
        resolve_upstream_fallback_base_url("https://api.openai.com/v1").as_deref(),
        None
    );

    std::env::remove_var("CODEXMANAGER_UPSTREAM_FALLBACK_BASE_URL");
    reload_from_env();
    assert!(!is_upstream_fallback_explicitly_configured());
}
