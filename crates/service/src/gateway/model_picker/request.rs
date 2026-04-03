use codexmanager_core::storage::{Account, Storage, Token};
use reqwest::blocking::Client;
use reqwest::header::HeaderMap;
use reqwest::header::CONTENT_TYPE;
use reqwest::Method;
use reqwest::StatusCode;
const REQUEST_ID_HEADER: &str = "x-request-id";
const OAI_REQUEST_ID_HEADER: &str = "x-oai-request-id";
const CF_RAY_HEADER: &str = "cf-ray";
const AUTH_ERROR_HEADER: &str = "x-openai-authorization-error";

fn append_client_version_query(url: &str) -> String {
    if url.contains("client_version=") {
        return url.to_string();
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    format!(
        "{url}{separator}client_version={}",
        super::super::upstream::header_profile::CODEX_CLIENT_VERSION
    )
}

fn build_models_request_headers(
    bearer: &str,
    user_agent: &str,
    originator: &str,
    residency_requirement: Option<&str>,
    include_account_header: bool,
    account_header_value: Option<&str>,
) -> Vec<(String, String)> {
    let mut headers = Vec::with_capacity(6);
    headers.push(("Accept".to_string(), "application/json".to_string()));
    headers.push(("User-Agent".to_string(), user_agent.to_string()));
    headers.push(("originator".to_string(), originator.to_string()));
    if let Some(residency_requirement) = residency_requirement
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        headers.push((
            crate::gateway::runtime_config::RESIDENCY_HEADER_NAME.to_string(),
            residency_requirement.to_string(),
        ));
    }
    headers.push(("Authorization".to_string(), format!("Bearer {}", bearer)));
    if include_account_header {
        if let Some(account_id) = account_header_value
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            headers.push(("ChatGPT-Account-ID".to_string(), account_id.to_string()));
        }
    }
    headers
}

fn extract_response_header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn summarize_models_error_response(
    status: StatusCode,
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
        super::super::http_bridge::summarize_upstream_error_hint_from_body(403, body.as_bytes())
    } else {
        super::super::http_bridge::summarize_upstream_error_hint_from_body(
            status.as_u16(),
            body.as_bytes(),
        )
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
        details.push(format!("identity_error_code: {identity_error_code}"));
    }

    if details.is_empty() {
        format!("models upstream failed: status={} body={body_hint}", status)
    } else {
        format!(
            "models upstream failed: status={} body={body_hint}, {}",
            status,
            details.join(", ")
        )
    }
}

pub(super) fn send_models_request(
    client: &Client,
    storage: &Storage,
    method: &Method,
    upstream_base: &str,
    path: &str,
    account: &Account,
    token: &mut Token,
) -> Result<Vec<u8>, String> {
    let (url, _url_alt) = super::super::compute_upstream_url(upstream_base, path);
    let url = append_client_version_query(&url);
    // 中文注释：OpenAI 基线要求 api_key_access_token，
    // 不这样区分会导致模型列表请求在 OpenAI 上游稳定 401。
    let bearer = if super::super::is_openai_api_base(upstream_base) {
        super::super::resolve_openai_bearer_token(storage, account, token)?
    } else {
        token.access_token.clone()
    };
    let account_header_value = account
        .chatgpt_account_id
        .as_deref()
        .or_else(|| account.workspace_id.as_deref())
        .map(str::to_string);
    let include_account_header = !super::super::is_openai_api_base(upstream_base);
    let build_request = |http: &Client| {
        let mut builder = http.request(method.clone(), &url);
        for (name, value) in build_models_request_headers(
            bearer.as_str(),
            crate::gateway::current_codex_user_agent().as_str(),
            crate::gateway::current_wire_originator().as_str(),
            crate::gateway::current_residency_requirement().as_deref(),
            include_account_header,
            account_header_value.as_deref(),
        ) {
            builder = builder.header(name, value);
        }
        builder
    };

    let response = match build_request(client).send() {
        Ok(resp) => resp,
        Err(first_err) => {
            let fresh = super::super::fresh_upstream_client_for_account(account.id.as_str());
            match build_request(&fresh).send() {
                Ok(resp) => resp,
                Err(second_err) => {
                    return Err(format!(
                        "models upstream request failed: {}; retry_after_fresh_client: {}",
                        first_err, second_err
                    ));
                }
            }
        }
    };
    if !response.status().is_success() {
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.text().unwrap_or_default();
        return Err(summarize_models_error_response(
            status, &headers, &body, false,
        ));
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if super::super::is_html_content_type(content_type) {
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.text().unwrap_or_default();
        return Err(summarize_models_error_response(
            status, &headers, &body, true,
        ));
    }

    response
        .bytes()
        .map(|v| v.to_vec())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        append_client_version_query, build_models_request_headers, summarize_models_error_response,
    };
    use reqwest::header::{HeaderMap, HeaderValue};
    use reqwest::StatusCode;

    #[test]
    fn append_client_version_query_adds_missing_param() {
        let actual = append_client_version_query("https://example.com/backend-api/codex/models");
        assert_eq!(
            actual,
            "https://example.com/backend-api/codex/models?client_version=0.118.0"
        );
    }

    #[test]
    fn append_client_version_query_preserves_existing_query() {
        let actual =
            append_client_version_query("https://example.com/backend-api/codex/models?limit=20");
        assert_eq!(
            actual,
            "https://example.com/backend-api/codex/models?limit=20&client_version=0.118.0"
        );
    }

    #[test]
    fn append_client_version_query_does_not_duplicate_param() {
        let actual = append_client_version_query(
            "https://example.com/backend-api/codex/models?client_version=0.118.0",
        );
        assert_eq!(
            actual,
            "https://example.com/backend-api/codex/models?client_version=0.118.0"
        );
    }

    #[test]
    fn build_models_request_headers_match_codex_profile() {
        let headers = build_models_request_headers(
            "access-token",
            "codex_cli_rs/1.2.3 (Windows 11; x86_64) terminal",
            "codex_cli_rs",
            Some("us"),
            true,
            Some("acc_123"),
        );
        let find = |name: &str| {
            headers
                .iter()
                .find(|(header, _)| header == name)
                .map(|(_, value)| value.as_str())
        };

        assert_eq!(find("Accept"), Some("application/json"));
        assert_eq!(
            find("User-Agent"),
            Some("codex_cli_rs/1.2.3 (Windows 11; x86_64) terminal")
        );
        assert_eq!(find("originator"), Some("codex_cli_rs"));
        assert_eq!(find("Authorization"), Some("Bearer access-token"));
        assert!(find("Cookie").is_none());
        assert_eq!(find("ChatGPT-Account-ID"), Some("acc_123"));
        assert_eq!(
            find(crate::gateway::runtime_config::RESIDENCY_HEADER_NAME),
            Some("us")
        );
        assert!(find("Version").is_none());
        assert!(find("ChatGPT-Account-Id").is_none());
    }

    #[test]
    fn build_models_request_headers_omits_optional_headers_when_not_applicable() {
        let headers = build_models_request_headers(
            "access-token",
            "codex_cli_rs/1.2.3",
            "codex_cli_rs",
            None,
            false,
            Some("acc_123"),
        );
        let find = |name: &str| {
            headers
                .iter()
                .find(|(header, _)| header == name)
                .map(|(_, value)| value.as_str())
        };

        assert!(find("Cookie").is_none());
        assert!(find("ChatGPT-Account-ID").is_none());
        assert!(find(crate::gateway::runtime_config::RESIDENCY_HEADER_NAME).is_none());
    }

    #[test]
    fn summarize_models_error_response_uses_stable_challenge_hint_and_debug_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-oai-request-id", HeaderValue::from_static("req-models"));
        headers.insert("cf-ray", HeaderValue::from_static("ray-models"));
        headers.insert(
            "x-openai-authorization-error",
            HeaderValue::from_static("missing_authorization_header"),
        );
        headers.insert(
            "x-error-json",
            HeaderValue::from_static("{\"identity_error_code\":\"org_membership_required\"}"),
        );

        let message = summarize_models_error_response(
            StatusCode::FORBIDDEN,
            &headers,
            "<html><title>Just a moment...</title></html>",
            false,
        );

        assert!(message.contains("Cloudflare 安全验证页（title=Just a moment...）"));
        assert!(message.contains("request id: req-models"));
        assert!(message.contains("cf-ray: ray-models"));
        assert!(message.contains("auth error: missing_authorization_header"));
        assert!(message.contains("identity_error_code: org_membership_required"));
        assert!(!message.contains("<html>"));
    }

    #[test]
    fn summarize_models_error_response_includes_identity_error_code() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-error-json",
            HeaderValue::from_static("{\"identity_error_code\":\"access_denied\"}"),
        );

        let message = summarize_models_error_response(
            StatusCode::FORBIDDEN,
            &headers,
            "{\"error\":{\"message\":\"blocked\"}}",
            false,
        );

        assert!(message.contains("identity_error_code: access_denied"));
    }
}
