use super::{
    build_usage_request_headers, summarize_usage_error_response, usage_http_client,
    CHATGPT_ACCOUNT_ID_HEADER_NAME,
};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::StatusCode;
use std::sync::MutexGuard;

fn usage_header_runtime_guard() -> MutexGuard<'static, ()> {
    crate::gateway::gateway_runtime_test_guard()
}

fn usage_header_runtime_scope() -> (MutexGuard<'static, ()>, UsageHeaderRuntimeRestore) {
    let guard = usage_header_runtime_guard();
    let restore = UsageHeaderRuntimeRestore::capture();
    let _ = crate::set_gateway_originator("codex_cli_rs");
    let _ = crate::set_gateway_residency_requirement(None);
    (guard, restore)
}

struct UsageHeaderRuntimeRestore {
    originator: String,
    residency_requirement: Option<String>,
}

impl UsageHeaderRuntimeRestore {
    fn capture() -> Self {
        Self {
            originator: crate::current_gateway_originator(),
            residency_requirement: crate::current_gateway_residency_requirement(),
        }
    }
}

impl Drop for UsageHeaderRuntimeRestore {
    fn drop(&mut self) {
        let _ = crate::set_gateway_originator(&self.originator);
        let _ = crate::set_gateway_residency_requirement(self.residency_requirement.as_deref());
    }
}

#[test]
fn usage_http_client_is_cloneable() {
    let first = usage_http_client();
    let second = usage_http_client();
    let first_ptr = &first as *const reqwest::blocking::Client;
    let second_ptr = &second as *const reqwest::blocking::Client;
    assert_ne!(first_ptr, second_ptr);
}

#[test]
fn refresh_token_status_error_omits_empty_body() {
    assert_eq!(
        super::format_refresh_token_status_error(StatusCode::FORBIDDEN, "   "),
        "refresh token failed with status 403 Forbidden"
    );
}

#[test]
fn refresh_token_status_error_includes_body_snippet() {
    assert_eq!(
        super::format_refresh_token_status_error(
            StatusCode::BAD_REQUEST,
            "{\n  \"error\": \"invalid_grant\"\n}"
        ),
        "refresh token failed with status 400 Bad Request: invalid_grant"
    );
}

#[test]
fn refresh_token_status_error_maps_invalidated_401_to_official_message() {
    assert_eq!(
        super::format_refresh_token_status_error(
            StatusCode::UNAUTHORIZED,
            "{\"error\":\"refresh_token_invalidated\"}"
        ),
        "refresh token failed with status 401 Unauthorized: Your access token could not be refreshed because your refresh token was revoked. Please log out and sign in again."
    );
}

#[test]
fn refresh_token_status_error_maps_unknown_401_to_official_message() {
    assert_eq!(
        super::format_refresh_token_status_error(
            StatusCode::UNAUTHORIZED,
            "{\"error\":\"something_else\"}"
        ),
        "refresh token failed with status 401 Unauthorized: Your access token could not be refreshed. Please log out and sign in again."
    );
}

#[test]
fn classify_refresh_token_auth_error_reason_maps_known_and_unknown_401() {
    assert_eq!(
        super::classify_refresh_token_auth_error_reason(
            StatusCode::UNAUTHORIZED,
            "{\"error\":\"refresh_token_invalidated\"}"
        ),
        Some(super::RefreshTokenAuthErrorReason::Invalidated)
    );
    assert_eq!(
        super::classify_refresh_token_auth_error_reason(
            StatusCode::UNAUTHORIZED,
            "{\"error\":\"something_else\"}"
        ),
        Some(super::RefreshTokenAuthErrorReason::Unknown401)
    );
    assert_eq!(
        super::classify_refresh_token_auth_error_reason(
            StatusCode::FORBIDDEN,
            "{\"error\":\"refresh_token_invalidated\"}"
        ),
        None
    );
}

#[test]
fn refresh_token_status_error_ignores_headers_for_401_reason_when_body_lacks_code() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-error-json",
        HeaderValue::from_static("{\"identity_error_code\":\"refresh_token_invalidated\"}"),
    );
    headers.insert(
        "x-openai-authorization-error",
        HeaderValue::from_static("refresh_token_expired"),
    );

    assert_eq!(
        super::format_refresh_token_status_error_with_headers(
            StatusCode::UNAUTHORIZED,
            Some(&headers),
            "<html><title>Just a moment...</title></html>"
        ),
        "refresh token failed with status 401 Unauthorized: Your access token could not be refreshed. Please log out and sign in again."
    );
}

#[test]
fn refresh_token_status_error_stabilizes_html_and_debug_headers_for_non_401() {
    let mut headers = HeaderMap::new();
    headers.insert("x-request-id", HeaderValue::from_static("req_refresh_123"));
    headers.insert("cf-ray", HeaderValue::from_static("cf_refresh_123"));
    headers.insert(
        "x-openai-authorization-error",
        HeaderValue::from_static("missing_authorization_header"),
    );
    headers.insert(
        "x-error-json",
        HeaderValue::from_static("{\"identity_error_code\":\"token_expired\"}"),
    );

    let message = super::format_refresh_token_status_error_with_headers(
        StatusCode::FORBIDDEN,
        Some(&headers),
        "<html><head><title>Just a moment...</title></head><body>challenge</body></html>",
    );

    assert!(message.contains("refresh token failed with status 403 Forbidden"));
    assert!(message.contains("Cloudflare 安全验证页"));
    assert!(message.contains("kind=cloudflare_challenge"));
    assert!(message.contains("request_id=req_refresh_123"));
    assert!(message.contains("cf_ray=cf_refresh_123"));
    assert!(message.contains("auth_error=missing_authorization_header"));
    assert!(message.contains("identity_error_code=token_expired"));
}

#[test]
fn refresh_token_status_error_uses_header_only_debug_suffix_for_empty_body() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-request-id",
        HeaderValue::from_static("req_refresh_empty"),
    );
    headers.insert("cf-ray", HeaderValue::from_static("cf_refresh_empty"));

    let message = super::format_refresh_token_status_error_with_headers(
        StatusCode::BAD_GATEWAY,
        Some(&headers),
        "",
    );

    assert!(message.contains("refresh token failed with status 502 Bad Gateway"));
    assert!(message.contains("kind=cloudflare_edge"));
    assert!(message.contains("request_id=req_refresh_empty"));
    assert!(message.contains("cf_ray=cf_refresh_empty"));
}

#[test]
fn refresh_token_auth_error_reason_from_message_tracks_canonical_messages() {
    let invalidated = super::format_refresh_token_status_error(
        StatusCode::UNAUTHORIZED,
        "{\"error\":\"refresh_token_invalidated\"}",
    );
    assert_eq!(
        super::refresh_token_auth_error_reason_from_message(&invalidated),
        Some(super::RefreshTokenAuthErrorReason::Invalidated)
    );

    let unknown = super::format_refresh_token_status_error(
        StatusCode::UNAUTHORIZED,
        "{\"error\":\"something_else\"}",
    );
    assert_eq!(
        super::refresh_token_auth_error_reason_from_message(&unknown),
        Some(super::RefreshTokenAuthErrorReason::Unknown401)
    );
}

#[test]
fn usage_http_default_headers_follow_gateway_runtime_profile() {
    let (_guard, _restore) = usage_header_runtime_scope();
    crate::set_gateway_originator("codex_cli_rs_usage").expect("set gateway originator");
    crate::set_gateway_residency_requirement(Some("us"))
        .expect("set gateway residency requirement");

    let headers = super::build_usage_http_default_headers();

    assert_eq!(
        headers
            .get("originator")
            .and_then(|value| value.to_str().ok()),
        Some("codex_cli_rs_usage")
    );
    assert_eq!(
        headers
            .get("x-openai-internal-codex-residency")
            .and_then(|value| value.to_str().ok()),
        Some("us")
    );
}

#[test]
fn usage_request_headers_use_official_chatgpt_account_header_name() {
    let headers = build_usage_request_headers(Some("workspace_123"));

    assert_eq!(
        headers
            .get(CHATGPT_ACCOUNT_ID_HEADER_NAME)
            .and_then(|value| value.to_str().ok()),
        Some("workspace_123")
    );
    assert_eq!(headers.len(), 1);
}

#[test]
fn refresh_token_url_uses_official_default_for_openai_issuer() {
    std::env::remove_var("CODEX_REFRESH_TOKEN_URL_OVERRIDE");

    assert_eq!(
        super::resolve_refresh_token_url("https://auth.openai.com"),
        "https://auth.openai.com/oauth/token"
    );
    assert_eq!(
        super::resolve_refresh_token_url("https://auth.openai.com/"),
        "https://auth.openai.com/oauth/token"
    );
}

#[test]
fn refresh_token_url_preserves_custom_issuer_and_override() {
    let previous = std::env::var("CODEX_REFRESH_TOKEN_URL_OVERRIDE").ok();

    std::env::remove_var("CODEX_REFRESH_TOKEN_URL_OVERRIDE");
    assert_eq!(
        super::resolve_refresh_token_url("https://auth.example.com"),
        "https://auth.example.com/oauth/token"
    );

    std::env::set_var(
        "CODEX_REFRESH_TOKEN_URL_OVERRIDE",
        "https://override.example.com/custom/token",
    );
    assert_eq!(
        super::resolve_refresh_token_url("https://auth.example.com"),
        "https://override.example.com/custom/token"
    );

    match previous {
        Some(value) => std::env::set_var("CODEX_REFRESH_TOKEN_URL_OVERRIDE", value),
        None => std::env::remove_var("CODEX_REFRESH_TOKEN_URL_OVERRIDE"),
    }
}

#[test]
fn summarize_usage_error_response_stabilizes_html_and_debug_headers() {
    let mut headers = HeaderMap::new();
    headers.insert("x-request-id", HeaderValue::from_static("req_usage_123"));
    headers.insert("cf-ray", HeaderValue::from_static("cf_usage_123"));
    headers.insert(
        "x-openai-authorization-error",
        HeaderValue::from_static("missing_authorization_header"),
    );
    headers.insert(
        "x-error-json",
        HeaderValue::from_static("eyJlcnJvciI6eyJjb2RlIjoidG9rZW5fZXhwaXJlZCJ9fQ=="),
    );

    let summary = summarize_usage_error_response(
        StatusCode::FORBIDDEN,
        &headers,
        "<html><head><title>Just a moment...</title></head><body>challenge</body></html>",
        true,
    );

    assert!(summary.contains("usage endpoint failed: status=403 Forbidden"));
    assert!(summary.contains("Cloudflare 安全验证页"));
    assert!(summary.contains("request id: req_usage_123"));
    assert!(summary.contains("cf-ray: cf_usage_123"));
    assert!(summary.contains("auth error: missing_authorization_header"));
    assert!(summary.contains("identity error code: token_expired"));
}

#[test]
fn summarize_usage_error_response_accepts_raw_error_json_header() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-request-id",
        HeaderValue::from_static("req_usage_raw_123"),
    );
    headers.insert(
        "x-error-json",
        HeaderValue::from_static("{\"details\":{\"identity_error_code\":\"proxy_auth_required\"}}"),
    );

    let summary = summarize_usage_error_response(
        StatusCode::BAD_GATEWAY,
        &headers,
        "<html><head><title>502 Bad Gateway</title></head></html>",
        false,
    );

    assert!(summary.contains("request id: req_usage_raw_123"));
    assert!(summary.contains("identity error code: proxy_auth_required"));
    assert!(summary.contains("上游返回 HTML 错误页（title=502 Bad Gateway）"));
}
