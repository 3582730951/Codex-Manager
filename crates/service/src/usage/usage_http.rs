use codexmanager_core::usage::usage_endpoint;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};
use reqwest::Proxy;
use serde::Serialize;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

static USAGE_HTTP_CLIENT: OnceLock<RwLock<Client>> = OnceLock::new();
const USAGE_HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const ENV_UPSTREAM_PROXY_URL: &str = "CODEXMANAGER_UPSTREAM_PROXY_URL";
// NOTE: rely on reqwest built-in timeout (covers the full request including response body read).
// Avoid background worker threads + recv_timeout which cannot cancel the underlying read.
const USAGE_HTTP_TOTAL_TIMEOUT: Duration = Duration::from_secs(60);
const REFRESH_TOKEN_EXPIRED_MESSAGE: &str =
    "Your access token could not be refreshed because your refresh token has expired. Please log out and sign in again.";
const REFRESH_TOKEN_REUSED_MESSAGE: &str =
    "Your access token could not be refreshed because your refresh token was already used. Please log out and sign in again.";
const REFRESH_TOKEN_INVALIDATED_MESSAGE: &str =
    "Your access token could not be refreshed because your refresh token was revoked. Please log out and sign in again.";
const REFRESH_TOKEN_UNKNOWN_MESSAGE: &str =
    "Your access token could not be refreshed. Please log out and sign in again.";
const REFRESH_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR: &str = "CODEX_REFRESH_TOKEN_URL_OVERRIDE";
const RESIDENCY_HEADER_NAME: &str = "x-openai-internal-codex-residency";
const CHATGPT_ACCOUNT_ID_HEADER_NAME: &str = "ChatGPT-Account-ID";
const REQUEST_ID_HEADER: &str = "x-request-id";
const OAI_REQUEST_ID_HEADER: &str = "x-oai-request-id";
const CF_RAY_HEADER: &str = "cf-ray";
const AUTH_ERROR_HEADER: &str = "x-openai-authorization-error";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefreshTokenAuthErrorReason {
    Expired,
    Reused,
    Invalidated,
    Unknown401,
}

impl RefreshTokenAuthErrorReason {
    pub(crate) fn as_code(self) -> &'static str {
        match self {
            Self::Expired => "refresh_token_expired",
            Self::Reused => "refresh_token_reused",
            Self::Invalidated => "refresh_token_invalidated",
            Self::Unknown401 => "refresh_token_unknown_401",
        }
    }

    fn user_message(self) -> &'static str {
        match self {
            Self::Expired => REFRESH_TOKEN_EXPIRED_MESSAGE,
            Self::Reused => REFRESH_TOKEN_REUSED_MESSAGE,
            Self::Invalidated => REFRESH_TOKEN_INVALIDATED_MESSAGE,
            Self::Unknown401 => REFRESH_TOKEN_UNKNOWN_MESSAGE,
        }
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct RefreshTokenResponse {
    pub(crate) access_token: String,
    #[serde(default)]
    pub(crate) refresh_token: Option<String>,
    #[serde(default)]
    pub(crate) id_token: Option<String>,
}

#[derive(Serialize)]
struct RefreshTokenRequest<'a> {
    client_id: &'a str,
    grant_type: &'a str,
    refresh_token: &'a str,
}

fn extract_refresh_token_error_code(body: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
    value
        .get("error")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_ascii_lowercase())
        .or_else(|| {
            value
                .get("code")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .map(|value| value.to_ascii_lowercase())
        })
        .or_else(|| {
            value
                .get("error")
                .and_then(|value| value.get("code"))
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .map(|value| value.to_ascii_lowercase())
        })
}

fn extract_refresh_token_error_code_from_headers(headers: &HeaderMap) -> Option<String> {
    crate::gateway::extract_identity_error_code_from_headers(headers).or_else(|| {
        extract_response_header(headers, AUTH_ERROR_HEADER)
            .map(|value| value.to_ascii_lowercase())
            .filter(|value| !value.trim().is_empty())
    })
}

fn classify_refresh_token_auth_error_reason_from_code(
    code: Option<&str>,
) -> RefreshTokenAuthErrorReason {
    match code {
        Some("refresh_token_expired") => RefreshTokenAuthErrorReason::Expired,
        Some("refresh_token_reused") => RefreshTokenAuthErrorReason::Reused,
        Some("refresh_token_invalidated") => RefreshTokenAuthErrorReason::Invalidated,
        _ => RefreshTokenAuthErrorReason::Unknown401,
    }
}

#[cfg(test)]
pub(crate) fn classify_refresh_token_auth_error_reason(
    status: reqwest::StatusCode,
    body: &str,
) -> Option<RefreshTokenAuthErrorReason> {
    classify_refresh_token_auth_error_reason_with_headers(status, None, body)
}

fn classify_refresh_token_auth_error_reason_with_headers(
    status: reqwest::StatusCode,
    headers: Option<&HeaderMap>,
    body: &str,
) -> Option<RefreshTokenAuthErrorReason> {
    if status != reqwest::StatusCode::UNAUTHORIZED {
        return None;
    }
    let header_code = headers.and_then(extract_refresh_token_error_code_from_headers);
    Some(classify_refresh_token_auth_error_reason_from_code(
        extract_refresh_token_error_code(body)
            .or(header_code)
            .as_deref(),
    ))
}

pub(crate) fn refresh_token_auth_error_reason_from_message(
    message: &str,
) -> Option<RefreshTokenAuthErrorReason> {
    let normalized = message.trim();
    if !normalized.contains("refresh token failed with status 401") {
        return None;
    }
    if normalized.contains(REFRESH_TOKEN_EXPIRED_MESSAGE) {
        return Some(RefreshTokenAuthErrorReason::Expired);
    }
    if normalized.contains(REFRESH_TOKEN_REUSED_MESSAGE) {
        return Some(RefreshTokenAuthErrorReason::Reused);
    }
    if normalized.contains(REFRESH_TOKEN_INVALIDATED_MESSAGE) {
        return Some(RefreshTokenAuthErrorReason::Invalidated);
    }
    Some(RefreshTokenAuthErrorReason::Unknown401)
}

#[cfg(test)]
fn format_refresh_token_status_error(status: reqwest::StatusCode, body: &str) -> String {
    format_refresh_token_status_error_with_headers(status, None, body)
}

fn format_refresh_token_status_error_with_headers(
    status: reqwest::StatusCode,
    headers: Option<&HeaderMap>,
    body: &str,
) -> String {
    if let Some(reason) =
        classify_refresh_token_auth_error_reason_with_headers(status, headers, body)
    {
        let message = reason.user_message();
        return format!("refresh token failed with status {status}: {message}");
    }

    let snippet = body
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(256)
        .collect::<String>();
    if snippet.is_empty() {
        format!("refresh token failed with status {status}")
    } else {
        format!("refresh token failed with status {status}: {snippet}")
    }
}

fn build_usage_http_client() -> Client {
    let default_headers = build_usage_http_default_headers();
    let mut builder = Client::builder()
        // 中文注释：轮询链路复用连接池可降低握手开销；不复用会在多账号刷新时放大短连接抖动。
        .connect_timeout(USAGE_HTTP_CONNECT_TIMEOUT)
        .timeout(USAGE_HTTP_TOTAL_TIMEOUT)
        .pool_max_idle_per_host(8)
        .pool_idle_timeout(Some(Duration::from_secs(60)))
        .user_agent(crate::gateway::current_codex_user_agent())
        .default_headers(default_headers);
    if let Some(proxy_url) = current_upstream_proxy_url() {
        match Proxy::all(proxy_url.as_str()) {
            Ok(proxy) => {
                builder = builder.proxy(proxy);
            }
            Err(err) => {
                log::warn!(
                    "event=usage_http_proxy_invalid proxy={} err={}",
                    proxy_url,
                    err
                );
            }
        }
    }
    builder.build().unwrap_or_else(|_| Client::new())
}

fn build_usage_http_default_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&crate::gateway::current_originator()) {
        headers.insert(HeaderName::from_static("originator"), value);
    }
    if let Some(residency_requirement) = crate::gateway::current_residency_requirement() {
        if let Ok(value) = HeaderValue::from_str(&residency_requirement) {
            headers.insert(HeaderName::from_static(RESIDENCY_HEADER_NAME), value);
        }
    }
    headers
}

fn build_usage_request_headers(workspace_id: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(workspace_id) = workspace_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Ok(value) = HeaderValue::from_str(workspace_id) {
            if let Ok(name) = HeaderName::from_bytes(CHATGPT_ACCOUNT_ID_HEADER_NAME.as_bytes()) {
                headers.insert(name, value);
            }
        }
    }
    headers
}

fn resolve_refresh_token_url(issuer: &str) -> String {
    if let Some(override_url) = std::env::var(REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return override_url;
    }

    let normalized_issuer = issuer.trim().trim_end_matches('/');
    if normalized_issuer.is_empty()
        || normalized_issuer.eq_ignore_ascii_case("https://auth.openai.com")
    {
        return REFRESH_TOKEN_URL.to_string();
    }

    format!("{normalized_issuer}/oauth/token")
}

fn extract_response_header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn summarize_usage_error_response(
    status: reqwest::StatusCode,
    headers: &HeaderMap,
    body: &str,
    force_html_error: bool,
) -> String {
    let request_id = extract_response_header(headers, REQUEST_ID_HEADER)
        .or_else(|| extract_response_header(headers, OAI_REQUEST_ID_HEADER));
    let cf_ray = extract_response_header(headers, CF_RAY_HEADER);
    let auth_error = extract_response_header(headers, AUTH_ERROR_HEADER);
    let identity_error_code = crate::gateway::extract_identity_error_code_from_headers(headers);
    let body_hint = if force_html_error {
        crate::gateway::summarize_upstream_error_hint_from_body(403, body.as_bytes())
    } else {
        crate::gateway::summarize_upstream_error_hint_from_body(status.as_u16(), body.as_bytes())
    }
    .or_else(|| {
        let trimmed = body.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
    .unwrap_or_else(|| "unknown error".to_string());

    let mut details = Vec::new();
    if let Some(request_id) = request_id {
        details.push(format!("request id: {request_id}"));
    }
    if let Some(cf_ray) = cf_ray {
        details.push(format!("cf-ray: {cf_ray}"));
    }
    if let Some(auth_error) = auth_error {
        details.push(format!("auth error: {auth_error}"));
    }
    if let Some(identity_error_code) = identity_error_code {
        details.push(format!("identity error code: {identity_error_code}"));
    }

    if details.is_empty() {
        format!("usage endpoint failed: status={} body={body_hint}", status)
    } else {
        format!(
            "usage endpoint failed: status={} body={body_hint}, {}",
            status,
            details.join(", ")
        )
    }
}

pub(crate) fn usage_http_client() -> Client {
    let lock = USAGE_HTTP_CLIENT.get_or_init(|| RwLock::new(build_usage_http_client()));
    crate::lock_utils::read_recover(lock, "usage_http_client").clone()
}

fn rebuild_usage_http_client() {
    let next = build_usage_http_client();
    let lock = USAGE_HTTP_CLIENT.get_or_init(|| RwLock::new(next.clone()));
    let mut current = crate::lock_utils::write_recover(lock, "usage_http_client");
    *current = next;
}

pub(crate) fn reload_usage_http_client_from_env() {
    rebuild_usage_http_client();
}

fn current_upstream_proxy_url() -> Option<String> {
    std::env::var(ENV_UPSTREAM_PROXY_URL)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn fetch_usage_snapshot(
    base_url: &str,
    bearer: &str,
    workspace_id: Option<&str>,
) -> Result<serde_json::Value, String> {
    // 调用上游用量接口
    let url = usage_endpoint(base_url);
    let build_request = || {
        let client = usage_http_client();
        let mut req = client
            .get(&url)
            .header("Authorization", format!("Bearer {bearer}"));
        let request_headers = build_usage_request_headers(workspace_id);
        if !request_headers.is_empty() {
            req = req.headers(request_headers);
        }
        req
    };
    let resp = match build_request().send() {
        Ok(resp) => resp,
        Err(first_err) => {
            // 中文注释：代理在程序启动后才开启时，旧 client 可能沿用旧网络状态；这里自动重建并重试一次。
            rebuild_usage_http_client();
            let retried = build_request().send();
            match retried {
                Ok(resp) => resp,
                Err(second_err) => {
                    return Err(format!(
                        "{}; retry_after_client_rebuild: {}",
                        first_err, second_err
                    ));
                }
            }
        }
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.text().unwrap_or_default();
        return Err(summarize_usage_error_response(
            status, &headers, &body, false,
        ));
    }
    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if crate::gateway::is_html_content_type(content_type) {
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.text().unwrap_or_default();
        return Err(summarize_usage_error_response(
            status, &headers, &body, true,
        ));
    }
    resp.json::<serde_json::Value>()
        .map_err(|e| format!("read usage endpoint json failed: {e}"))
}

pub(crate) fn refresh_access_token(
    issuer: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<RefreshTokenResponse, String> {
    let refresh_token_url = resolve_refresh_token_url(issuer);
    let body = RefreshTokenRequest {
        client_id,
        grant_type: "refresh_token",
        refresh_token,
    };
    let build_request = || {
        let client = usage_http_client();
        client
            .post(refresh_token_url.clone())
            .header("Content-Type", "application/json")
            .json(&body)
    };
    let resp = match build_request().send() {
        Ok(resp) => resp,
        Err(first_err) => {
            rebuild_usage_http_client();
            let retried = build_request().send();
            match retried {
                Ok(resp) => resp,
                Err(second_err) => {
                    return Err(format!(
                        "{}; retry_after_client_rebuild: {}",
                        first_err, second_err
                    ));
                }
            }
        }
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.text().unwrap_or_default();
        return Err(format_refresh_token_status_error_with_headers(
            status,
            Some(&headers),
            body.as_str(),
        ));
    }
    resp.json::<RefreshTokenResponse>()
        .map_err(|e| format!("read refresh token response json failed: {e}"))
}

#[cfg(test)]
#[path = "tests/usage_http_tests.rs"]
mod tests;
