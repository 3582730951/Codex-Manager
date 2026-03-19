use super::{should_failover_after_fallback_non_success, should_trigger_openai_fallback};
use reqwest::header::HeaderValue;
use reqwest::StatusCode;

#[test]
fn fallback_non_success_5xx_does_not_failover_even_with_more_candidates() {
    assert!(!should_failover_after_fallback_non_success(500, true));
    assert!(!should_failover_after_fallback_non_success(503, true));
}

#[test]
fn fallback_non_success_auth_and_rate_limit_can_failover_when_candidates_remain() {
    assert!(should_failover_after_fallback_non_success(401, true));
    assert!(should_failover_after_fallback_non_success(403, true));
    assert!(should_failover_after_fallback_non_success(404, true));
    assert!(should_failover_after_fallback_non_success(429, true));
}

#[test]
fn fallback_non_success_never_failover_without_more_candidates() {
    assert!(!should_failover_after_fallback_non_success(401, false));
    assert!(!should_failover_after_fallback_non_success(429, false));
    assert!(!should_failover_after_fallback_non_success(500, false));
}

#[test]
fn fallback_trigger_matches_responses_auth_rate_limit_and_html_challenge() {
    let html = HeaderValue::from_static("text/html; charset=utf-8");
    assert!(should_trigger_openai_fallback(
        "https://chatgpt.com/backend-api/codex",
        "/v1/responses",
        StatusCode::TOO_MANY_REQUESTS,
        None,
    ));
    assert!(should_trigger_openai_fallback(
        "https://chatgpt.com/backend-api/codex",
        "/v1/chat/completions",
        StatusCode::FORBIDDEN,
        Some(&html),
    ));
    assert!(!should_trigger_openai_fallback(
        "https://api.openai.com/v1",
        "/v1/responses",
        StatusCode::TOO_MANY_REQUESTS,
        None,
    ));
}
