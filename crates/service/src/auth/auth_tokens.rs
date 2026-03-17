use codexmanager_core::auth::{
    extract_chatgpt_account_id, extract_workspace_id, parse_id_token_claims,
    token_exchange_body_authorization_code, token_exchange_body_token_exchange, IdTokenClaims,
    DEFAULT_CLIENT_ID, DEFAULT_ISSUER,
};
use codexmanager_core::storage::{now_ts, Account, Token};
use reqwest::blocking::Client;
use reqwest::header::HeaderMap;
use reqwest::Error as ReqwestError;
use serde::de::DeserializeOwned;
use std::sync::mpsc;
use std::time::Duration;

use crate::account_identity::{
    build_account_storage_id, build_fallback_subject_key, clean_value,
    pick_existing_account_id_by_identity,
};
use crate::auth_callback::resolve_redirect_uri;
use crate::storage_helpers::open_storage;

static OPENAI_AUTH_HTTP_CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
const OPENAI_AUTH_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const OPENAI_AUTH_READ_TIMEOUT: Duration = Duration::from_secs(30);
const OPENAI_AUTH_TOTAL_TIMEOUT: Duration = Duration::from_secs(60);
const ACCOUNT_SORT_STEP: i64 = 5;
const REQUEST_ID_HEADER: &str = "x-request-id";
const OAI_REQUEST_ID_HEADER: &str = "x-oai-request-id";
const CF_RAY_HEADER: &str = "cf-ray";
const AUTH_ERROR_HEADER: &str = "x-openai-authorization-error";
const CLOUDFLARE_BLOCKED_MESSAGE: &str =
    "Access blocked by Cloudflare. This usually happens when connecting from a restricted region";

fn read_json_with_timeout<T>(
    resp: reqwest::blocking::Response,
    read_timeout: Duration,
) -> Result<T, String>
where
    T: DeserializeOwned + Send + 'static,
{
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = tx.send(resp.json::<T>().map_err(|e| e.to_string()));
    });
    match rx.recv_timeout(read_timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "response read timed out after {}ms",
            read_timeout.as_millis()
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("response read failed: worker disconnected".to_string())
        }
    }
}

fn read_text_with_timeout(
    resp: reqwest::blocking::Response,
    read_timeout: Duration,
) -> Result<String, String> {
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = tx.send(resp.text().map_err(|e| e.to_string()));
    });
    match rx.recv_timeout(read_timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "response read timed out after {}ms",
            read_timeout.as_millis()
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("response read failed: worker disconnected".to_string())
        }
    }
}

fn summarize_token_endpoint_error_body(body: &str) -> String {
    parse_token_endpoint_error(body).to_string()
}

fn extract_response_header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn build_token_endpoint_debug_suffix(headers: &HeaderMap) -> String {
    let request_id = extract_response_header(headers, REQUEST_ID_HEADER)
        .or_else(|| extract_response_header(headers, OAI_REQUEST_ID_HEADER));
    let cf_ray = extract_response_header(headers, CF_RAY_HEADER);
    let auth_error = extract_response_header(headers, AUTH_ERROR_HEADER);
    let identity_error_code = crate::gateway::extract_identity_error_code_from_headers(headers);

    let mut details = Vec::new();
    if let Some(request_id) = request_id {
        details.push(format!("request_id={request_id}"));
    }
    if let Some(cf_ray) = cf_ray {
        details.push(format!("cf_ray={cf_ray}"));
    }
    if let Some(auth_error) = auth_error {
        details.push(format!("auth_error={auth_error}"));
    }
    if let Some(identity_error_code) = identity_error_code {
        details.push(format!("identity_error_code={identity_error_code}"));
    }

    if details.is_empty() {
        String::new()
    } else {
        format!(" [{}]", details.join(", "))
    }
}

fn classify_token_endpoint_error_kind(body: &str) -> &'static str {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "empty";
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return "json";
    }
    let normalized = trimmed.to_ascii_lowercase();
    if normalized.contains("<html") || normalized.contains("<!doctype html") {
        if normalized.contains("cloudflare") && normalized.contains("blocked") {
            "cloudflare_blocked"
        } else if normalized.contains("cloudflare")
            || normalized.contains("just a moment")
            || normalized.contains("attention required")
        {
            "cloudflare_challenge"
        } else {
            "html"
        }
    } else {
        "non_json"
    }
}

fn looks_like_blocked_marker(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    normalized.contains("blocked")
        || normalized.contains("unsupported_country_region_territory")
        || normalized.contains("unsupported_country")
        || normalized.contains("region_restricted")
}

fn classify_token_endpoint_error_kind_with_headers(
    headers: &HeaderMap,
    body: &str,
) -> &'static str {
    let body_kind = classify_token_endpoint_error_kind(body);
    if !matches!(body_kind, "empty" | "non_json") {
        return body_kind;
    }

    if extract_response_header(headers, AUTH_ERROR_HEADER)
        .as_deref()
        .is_some_and(looks_like_blocked_marker)
        || crate::gateway::extract_identity_error_code_from_headers(headers)
            .as_deref()
            .is_some_and(looks_like_blocked_marker)
    {
        return "cloudflare_blocked";
    }

    if crate::gateway::extract_identity_error_code_from_headers(headers).is_some() {
        return "identity_error";
    }

    if extract_response_header(headers, AUTH_ERROR_HEADER).is_some() {
        return "auth_error";
    }

    if extract_response_header(headers, CF_RAY_HEADER).is_some() {
        return "cloudflare_edge";
    }

    body_kind
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TokenEndpointErrorDetail {
    error_code: Option<String>,
    error_message: Option<String>,
    display_message: String,
}

impl std::fmt::Display for TokenEndpointErrorDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.display_message.fmt(f)
    }
}

const REDACTED_URL_VALUE: &str = "<redacted>";
const SENSITIVE_URL_QUERY_KEYS: &[&str] = &[
    "access_token",
    "api_key",
    "client_secret",
    "code",
    "code_verifier",
    "id_token",
    "key",
    "refresh_token",
    "requested_token",
    "state",
    "subject_token",
    "token",
];

fn extract_html_title(raw: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let start = lower.find("<title>")?;
    let end = lower[start + 7..].find("</title>")? + start + 7;
    let title = raw.get(start + 7..end)?.trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

fn summarize_html_error_body(raw: &str) -> String {
    let normalized = raw.to_ascii_lowercase();
    let looks_like_blocked = normalized.contains("cloudflare") && normalized.contains("blocked");
    let looks_like_challenge = normalized.contains("cloudflare")
        || normalized.contains("just a moment")
        || normalized.contains("attention required");
    let looks_like_html = normalized.contains("<html")
        || normalized.contains("<!doctype html")
        || normalized.contains("</html>");
    if !looks_like_html {
        return raw.trim().to_string();
    }

    if looks_like_blocked {
        return CLOUDFLARE_BLOCKED_MESSAGE.to_string();
    }

    let title = extract_html_title(raw);
    if looks_like_challenge {
        return match title {
            Some(title) => format!("Cloudflare 安全验证页（title={title}）"),
            None => "Cloudflare 安全验证页".to_string(),
        };
    }

    match title {
        Some(title) => format!("上游返回 HTML 错误页（title={title}）"),
        None => "上游返回 HTML 错误页".to_string(),
    }
}

fn redact_sensitive_query_value(key: &str, value: &str) -> String {
    if SENSITIVE_URL_QUERY_KEYS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(key))
    {
        REDACTED_URL_VALUE.to_string()
    } else {
        value.to_string()
    }
}

fn redact_sensitive_url_parts(url: &mut url::Url) {
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_fragment(None);

    let query_pairs = url
        .query_pairs()
        .map(|(key, value)| {
            let key = key.into_owned();
            let value = value.into_owned();
            (key.clone(), redact_sensitive_query_value(&key, &value))
        })
        .collect::<Vec<_>>();

    if query_pairs.is_empty() {
        url.set_query(None);
        return;
    }

    let redacted_query = query_pairs
        .into_iter()
        .fold(
            url::form_urlencoded::Serializer::new(String::new()),
            |mut serializer, (key, value)| {
                serializer.append_pair(&key, &value);
                serializer
            },
        )
        .finish();
    url.set_query(Some(&redacted_query));
}

fn redact_sensitive_error_url(mut err: ReqwestError) -> ReqwestError {
    if let Some(url) = err.url_mut() {
        redact_sensitive_url_parts(url);
    }
    err
}

pub(crate) fn parse_token_endpoint_error(body: &str) -> TokenEndpointErrorDetail {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return TokenEndpointErrorDetail {
            error_code: None,
            error_message: None,
            display_message: "unknown error".to_string(),
        };
    }

    let parsed = serde_json::from_str::<serde_json::Value>(trimmed).ok();
    if let Some(json) = parsed {
        let error_code = json
            .get("error")
            .and_then(serde_json::Value::as_str)
            .filter(|error_code| !error_code.trim().is_empty())
            .map(ToString::to_string)
            .or_else(|| {
                json.get("error")
                    .and_then(serde_json::Value::as_object)
                    .and_then(|error_obj| error_obj.get("code"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|code| !code.trim().is_empty())
                    .map(ToString::to_string)
            });
        if let Some(description) = json
            .get("error_description")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            return TokenEndpointErrorDetail {
                error_code,
                error_message: Some(description.to_string()),
                display_message: description.to_string(),
            };
        }
        if let Some(message) = json
            .get("error")
            .and_then(serde_json::Value::as_object)
            .and_then(|error_obj| error_obj.get("message"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            return TokenEndpointErrorDetail {
                error_code,
                error_message: Some(message.to_string()),
                display_message: message.to_string(),
            };
        }
        if let Some(message) = json
            .get("message")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            return TokenEndpointErrorDetail {
                error_code,
                error_message: Some(message.to_string()),
                display_message: message.to_string(),
            };
        }
        if let Some(error_code) = error_code {
            return TokenEndpointErrorDetail {
                display_message: error_code.clone(),
                error_code: Some(error_code),
                error_message: None,
            };
        }
    }

    TokenEndpointErrorDetail {
        error_code: None,
        error_message: None,
        display_message: summarize_html_error_body(trimmed),
    }
}

fn summarize_header_only_token_endpoint_error(headers: &HeaderMap) -> Option<String> {
    if let Some(auth_error) = extract_response_header(headers, AUTH_ERROR_HEADER) {
        if looks_like_blocked_marker(&auth_error) {
            return Some(CLOUDFLARE_BLOCKED_MESSAGE.to_string());
        }
        return Some(format!("authorization error: {auth_error}"));
    }

    if let Some(identity_error_code) =
        crate::gateway::extract_identity_error_code_from_headers(headers)
    {
        if looks_like_blocked_marker(&identity_error_code) {
            return Some(CLOUDFLARE_BLOCKED_MESSAGE.to_string());
        }
        return Some(format!("identity error: {identity_error_code}"));
    }

    None
}

fn resolve_token_endpoint_error_detail(headers: &HeaderMap, body: &str) -> String {
    if !body.trim().is_empty() {
        return parse_token_endpoint_error(body).to_string();
    }

    summarize_header_only_token_endpoint_error(headers)
        .unwrap_or_else(|| parse_token_endpoint_error(body).to_string())
}

fn format_token_endpoint_status_error(
    status: reqwest::StatusCode,
    headers: &HeaderMap,
    body: &str,
) -> String {
    let detail = resolve_token_endpoint_error_detail(headers, body);
    let suffix = {
        let mut suffix = build_token_endpoint_debug_suffix(headers);
        let kind = classify_token_endpoint_error_kind_with_headers(headers, body);
        if kind != "json" {
            let addition = format!("kind={kind}");
            if suffix.is_empty() {
                suffix = format!(" [{addition}]");
            } else {
                suffix.insert_str(suffix.len() - 1, &format!(", {addition}"));
            }
        }
        suffix
    };
    format!("token endpoint returned status {status}: {detail}{suffix}")
}

fn format_api_key_exchange_status_error(
    status: reqwest::StatusCode,
    headers: &HeaderMap,
    body: &str,
) -> String {
    let detail = if body.trim().is_empty() {
        summarize_header_only_token_endpoint_error(headers)
            .unwrap_or_else(|| summarize_token_endpoint_error_body(body))
    } else {
        summarize_token_endpoint_error_body(body)
    };
    let suffix = {
        let mut suffix = build_token_endpoint_debug_suffix(headers);
        let kind = classify_token_endpoint_error_kind_with_headers(headers, body);
        if kind != "json" {
            let addition = format!("kind={kind}");
            if suffix.is_empty() {
                suffix = format!(" [{addition}]");
            } else {
                suffix.insert_str(suffix.len() - 1, &format!(", {addition}"));
            }
        }
        suffix
    };
    format!("api key exchange failed with status {status}: {detail}{suffix}")
}

pub(crate) fn next_account_sort(storage: &codexmanager_core::storage::Storage) -> i64 {
    storage
        .list_accounts()
        .ok()
        .and_then(|accounts| accounts.into_iter().map(|account| account.sort).max())
        .map(|sort| sort.saturating_add(ACCOUNT_SORT_STEP))
        .unwrap_or(0)
}

fn openai_auth_http_client() -> &'static Client {
    OPENAI_AUTH_HTTP_CLIENT.get_or_init(|| {
        Client::builder()
            .connect_timeout(OPENAI_AUTH_CONNECT_TIMEOUT)
            .timeout(OPENAI_AUTH_TOTAL_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new())
    })
}

pub(crate) fn complete_login(state: &str, code: &str) -> Result<(), String> {
    complete_login_with_redirect(state, code, None)
}

pub(crate) fn complete_login_with_redirect(
    state: &str,
    code: &str,
    redirect_uri: Option<&str>,
) -> Result<(), String> {
    // 读取登录会话
    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    let session = storage
        .get_login_session(state)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "unknown login session".to_string())?;

    // 读取 OAuth 配置
    let issuer =
        std::env::var("CODEXMANAGER_ISSUER").unwrap_or_else(|_| DEFAULT_ISSUER.to_string());
    let client_id =
        std::env::var("CODEXMANAGER_CLIENT_ID").unwrap_or_else(|_| DEFAULT_CLIENT_ID.to_string());
    let redirect_uri = redirect_uri
        .map(|value| value.to_string())
        .or_else(|| resolve_redirect_uri())
        .unwrap_or_else(|| "http://localhost:1455/auth/callback".to_string());

    // 交换授权码获取 token
    let tokens = exchange_code_for_tokens(
        &issuer,
        &client_id,
        &redirect_uri,
        &session.code_verifier,
        code,
    )
    .map_err(|e| {
        let _ = storage.update_login_session_status(state, "failed", Some(&e));
        e
    })?;

    // 可选兑换平台 key
    let api_key_access_token = obtain_api_key(&issuer, &client_id, &tokens.id_token).ok();
    let claims = parse_id_token_claims(&tokens.id_token).map_err(|e| {
        let _ = storage.update_login_session_status(state, "failed", Some(&e));
        e
    })?;
    if let Err(e) = ensure_workspace_allowed(
        session.workspace_id.as_deref(),
        &claims,
        &tokens.id_token,
        &tokens.access_token,
    ) {
        let _ = storage.update_login_session_status(state, "failed", Some(&e));
        return Err(e);
    }

    // 生成账户记录
    let subject_account_id = claims.sub.clone();
    let label = claims
        .email
        .clone()
        .unwrap_or_else(|| subject_account_id.clone());
    let chatgpt_account_id = clean_value(
        claims
            .auth
            .as_ref()
            .and_then(|auth| auth.chatgpt_account_id.clone())
            .or_else(|| extract_chatgpt_account_id(&tokens.id_token))
            .or_else(|| extract_chatgpt_account_id(&tokens.access_token)),
    );
    let workspace_id = clean_value(
        claims
            .workspace_id
            .clone()
            .or_else(|| extract_workspace_id(&tokens.id_token))
            .or_else(|| extract_workspace_id(&tokens.access_token))
            .or_else(|| chatgpt_account_id.clone()),
    );
    let fallback_subject_key =
        build_fallback_subject_key(Some(&subject_account_id), session.tags.as_deref());
    let account_storage_id = build_account_storage_id(
        &subject_account_id,
        chatgpt_account_id.as_deref(),
        workspace_id.as_deref(),
        session.tags.as_deref(),
    );
    let accounts = storage.list_accounts().map_err(|e| e.to_string())?;
    let account_key = pick_existing_account_id_by_identity(
        accounts.iter(),
        chatgpt_account_id.as_deref(),
        workspace_id.as_deref(),
        fallback_subject_key.as_deref(),
        None,
    )
    .unwrap_or(account_storage_id);
    let now = now_ts();
    let existing_account = storage
        .find_account_by_id(&account_key)
        .map_err(|e| e.to_string())?;
    let sort = existing_account
        .as_ref()
        .map(|account| account.sort)
        .unwrap_or_else(|| next_account_sort(&storage));
    let created_at = existing_account
        .as_ref()
        .map(|account| account.created_at)
        .unwrap_or(now);
    let account = Account {
        id: account_key.clone(),
        label,
        issuer: issuer.clone(),
        chatgpt_account_id,
        workspace_id,
        group_name: session.group_name.clone(),
        sort,
        status: "active".to_string(),
        created_at,
        updated_at: now,
    };
    storage
        .insert_account(&account)
        .map_err(|e| e.to_string())?;

    // 写入 token
    let token = Token {
        account_id: account_key.clone(),
        id_token: tokens.id_token,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        api_key_access_token,
        last_refresh: now,
    };
    storage.insert_token(&token).map_err(|e| e.to_string())?;

    storage
        .update_login_session_status(state, "success", None)
        .map_err(|e| e.to_string())?;
    crate::auth_account::set_current_auth_account_id(Some(&account_key))?;
    Ok(())
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    id_token: String,
    access_token: String,
    refresh_token: String,
}

fn exchange_code_for_tokens(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    code_verifier: &str,
    code: &str,
) -> Result<TokenResponse, String> {
    // 请求 token 接口
    let client = openai_auth_http_client();
    let resp = client
        .post(format!("{issuer}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(token_exchange_body_authorization_code(
            code,
            redirect_uri,
            client_id,
            code_verifier,
        ))
        .send()
        .map_err(|e| redact_sensitive_error_url(e).to_string())?;
    if !resp.status().is_success() {
        let status = resp.status();
        let headers = resp.headers().clone();
        let message = read_text_with_timeout(resp, OPENAI_AUTH_READ_TIMEOUT)
            .map(|body| format_token_endpoint_status_error(status, &headers, &body))
            .unwrap_or_else(|_| {
                let suffix = build_token_endpoint_debug_suffix(&headers);
                format!("token endpoint returned status {status}: unknown error{suffix}")
            });
        return Err(message);
    }
    read_json_with_timeout(resp, OPENAI_AUTH_READ_TIMEOUT)
}

pub(crate) fn obtain_api_key(
    issuer: &str,
    client_id: &str,
    id_token: &str,
) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct ExchangeResp {
        access_token: String,
    }

    // 兑换平台 API Key
    let client = openai_auth_http_client();
    let resp = client
        .post(format!("{issuer}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(token_exchange_body_token_exchange(id_token, client_id))
        .send()
        .map_err(|e| redact_sensitive_error_url(e).to_string())?;
    if !resp.status().is_success() {
        let status = resp.status();
        let headers = resp.headers().clone();
        let message = read_text_with_timeout(resp, OPENAI_AUTH_READ_TIMEOUT)
            .map(|body| format_api_key_exchange_status_error(status, &headers, &body))
            .unwrap_or_else(|_| {
                let suffix = build_token_endpoint_debug_suffix(&headers);
                format!("api key exchange failed with status {status}: unknown error{suffix}")
            });
        return Err(message);
    }
    let body: ExchangeResp = read_json_with_timeout(resp, OPENAI_AUTH_READ_TIMEOUT)?;
    Ok(body.access_token)
}

fn ensure_workspace_allowed(
    expected: Option<&str>,
    claims: &IdTokenClaims,
    id_token: &str,
    access_token: &str,
) -> Result<(), String> {
    let Some(expected) = expected.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(());
    };

    let actual = clean_value(
        claims
            .auth
            .as_ref()
            .and_then(|auth| auth.chatgpt_account_id.clone())
            .or_else(|| extract_chatgpt_account_id(id_token))
            .or_else(|| extract_chatgpt_account_id(access_token))
            .or_else(|| claims.workspace_id.clone())
            .or_else(|| extract_workspace_id(id_token))
            .or_else(|| extract_workspace_id(access_token)),
    );

    let Some(actual) = actual else {
        return Err("Login is restricted to a specific workspace, but the token did not include a workspace claim.".to_string());
    };

    if actual == expected {
        Ok(())
    } else {
        Err(format!("Login is restricted to workspace id {expected}."))
    }
}

#[cfg(test)]
#[path = "tests/auth_tokens_tests.rs"]
mod tests;
