use codexmanager_core::rpc::types::{
    AggregateApiCreateResult, AggregateApiSecretResult, AggregateApiSummary, AggregateApiTestResult,
};
use codexmanager_core::storage::{now_ts, AggregateApi};
use reqwest::header::{HeaderName, HeaderValue};
use serde_json::json;
use std::io::Read;
use std::time::Instant;

use crate::apikey_profile::normalize_upstream_base_url;
use crate::gateway;
use crate::storage_helpers::{generate_aggregate_api_id, open_storage};

pub(crate) const AGGREGATE_API_PROVIDER_CODEX: &str = "codex";
pub(crate) const AGGREGATE_API_PROVIDER_CLAUDE: &str = "claude";

fn normalize_secret(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_supplier_name(value: Option<String>) -> Result<String, String> {
    let normalized = value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "supplier name is required".to_string())?;
    Ok(normalized)
}

fn normalize_sort(value: Option<i64>) -> i64 {
    value.unwrap_or(0)
}

fn normalize_provider_type(value: Option<String>) -> Result<String, String> {
    match value {
        Some(raw) => {
            let normalized = raw.trim().to_ascii_lowercase().replace('-', "_");
            match normalized.as_str() {
                "codex" | "openai" | "openai_compat" | "gpt" => {
                    Ok(AGGREGATE_API_PROVIDER_CODEX.to_string())
                }
                "claude" | "anthropic" | "anthropic_native" | "claude_code" => {
                    Ok(AGGREGATE_API_PROVIDER_CLAUDE.to_string())
                }
                other => Err(format!("unsupported aggregate api provider type: {other}")),
            }
        }
        None => Ok(AGGREGATE_API_PROVIDER_CODEX.to_string()),
    }
}

fn normalize_provider_type_value(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "claude" | "anthropic" | "anthropic_native" | "claude_code" => {
            AGGREGATE_API_PROVIDER_CLAUDE.to_string()
        }
        _ => AGGREGATE_API_PROVIDER_CODEX.to_string(),
    }
}

fn provider_default_url(provider_type: &str) -> &'static str {
    match provider_type {
        AGGREGATE_API_PROVIDER_CLAUDE => "https://api.anthropic.com/v1",
        _ => "https://api.openai.com/v1",
    }
}

fn normalize_probe_url(base_url: &str, suffix: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}{suffix}")
    } else {
        format!("{base}/v1{suffix}")
    }
}

fn read_first_chunk(mut response: reqwest::blocking::Response) -> Result<(), String> {
    let mut buf = [0u8; 16];
    let read = response.read(&mut buf).map_err(|err| err.to_string())?;
    if read > 0 {
        Ok(())
    } else {
        Err("No response data received".to_string())
    }
}

fn build_claude_probe_body() -> serde_json::Value {
    json!({
        "model": "claude-haiku-4-5-20251001",
        "max_tokens": 1,
        "messages": [{
            "role": "user",
            "content": "Who are you?"
        }],
        "stream": true
    })
}

fn build_codex_probe_body() -> serde_json::Value {
    json!({
        "model": "gpt-5.1-codex",
        "input": [{
            "role": "user",
            "content": [{
                "type": "text",
                "text": "Who are you?"
            }]
        }],
        "stream": true
    })
}

fn append_client_version_query(url: &str) -> String {
    if url.contains("client_version=") {
        return url.to_string();
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    format!(
        "{url}{separator}client_version={}",
        gateway::current_codex_user_agent_version()
    )
}

fn probe_codex_only_for_provider(provider_type: &str) -> bool {
    provider_type != AGGREGATE_API_PROVIDER_CLAUDE
}

fn add_codex_probe_headers(
    builder: reqwest::blocking::RequestBuilder,
    secret: &str,
) -> Result<reqwest::blocking::RequestBuilder, String> {
    let auth_value = format!("Bearer {}", secret.trim());
    Ok(builder
        .header(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(auth_value.as_str())
                .map_err(|_| "invalid aggregate api key".to_string())?,
        )
        .header("x-api-key", secret.trim())
        .header("api-key", secret.trim())
        .header("accept", "application/json")
        .header("user-agent", gateway::current_codex_user_agent())
        .header("originator", gateway::current_wire_originator())
        .header("accept-encoding", "identity"))
}

fn probe_codex_models_endpoint(
    client: &reqwest::blocking::Client,
    base_url: &str,
    secret: &str,
) -> Result<i64, String> {
    let url = append_client_version_query(&normalize_probe_url(base_url, "/models"));
    let response = add_codex_probe_headers(client.get(url), secret)?
        .send()
        .map_err(|err| err.to_string())?;

    let status_code = response.status().as_u16() as i64;
    if !response.status().is_success() {
        return Err(format!("codex models probe http_status={status_code}"));
    }
    read_first_chunk(response)?;
    Ok(status_code)
}

fn probe_codex_responses_endpoint(
    client: &reqwest::blocking::Client,
    base_url: &str,
    secret: &str,
) -> Result<i64, String> {
    let url = normalize_probe_url(base_url, "/responses");
    let response = add_codex_probe_headers(client.post(url), secret)?
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .json(&build_codex_probe_body())
        .send()
        .map_err(|err| err.to_string())?;

    let status_code = response.status().as_u16() as i64;
    if !response.status().is_success() {
        return Err(format!("codex probe http_status={status_code}"));
    }
    read_first_chunk(response)?;
    Ok(status_code)
}

fn probe_codex_endpoint(
    client: &reqwest::blocking::Client,
    base_url: &str,
    secret: &str,
) -> Result<i64, String> {
    let models_result = probe_codex_models_endpoint(client, base_url, secret);
    if let Ok(code) = models_result {
        return Ok(code);
    }

    let models_err = models_result
        .err()
        .unwrap_or_else(|| "codex models probe failed".to_string());
    let responses_result = probe_codex_responses_endpoint(client, base_url, secret);
    if let Ok(code) = responses_result {
        return Ok(code);
    }

    let responses_err = responses_result
        .err()
        .unwrap_or_else(|| "codex responses probe failed".to_string());
    Err(format!("{models_err}; {responses_err}"))
}

fn probe_claude_endpoint(
    client: &reqwest::blocking::Client,
    base_url: &str,
    secret: &str,
) -> Result<i64, String> {
    let url = normalize_probe_url(base_url, "/messages?beta=true");
    let auth_value = format!("Bearer {}", secret.trim());
    let response = client
        .post(url)
        .header(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(auth_value.as_str())
                .map_err(|_| "invalid aggregate api key".to_string())?,
        )
        .header("x-api-key", secret.trim())
        .header("anthropic-version", "2023-06-01")
        .header(
            "anthropic-beta",
            "claude-code-20250219,interleaved-thinking-2025-05-14",
        )
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .header("accept-encoding", "identity")
        .header("user-agent", "claude-cli/2.1.2 (external, cli)")
        .header("x-app", "cli")
        .json(&build_claude_probe_body())
        .send()
        .map_err(|err| err.to_string())?;

    let status_code = response.status().as_u16() as i64;
    if !response.status().is_success() {
        return Err(format!("claude probe http_status={status_code}"));
    }
    read_first_chunk(response)?;
    Ok(status_code)
}

pub(crate) fn list_aggregate_apis() -> Result<Vec<AggregateApiSummary>, String> {
    let storage = open_storage().ok_or_else(|| "open storage failed".to_string())?;
    let items = storage
        .list_aggregate_apis()
        .map_err(|err| format!("list aggregate apis failed: {err}"))?;
    Ok(items
        .into_iter()
        .map(|item| AggregateApiSummary {
            id: item.id,
            provider_type: item.provider_type,
            supplier_name: item.supplier_name,
            sort: item.sort,
            url: item.url,
            status: item.status,
            created_at: item.created_at,
            updated_at: item.updated_at,
            last_test_at: item.last_test_at,
            last_test_status: item.last_test_status,
            last_test_error: item.last_test_error,
        })
        .collect())
}

pub(crate) fn create_aggregate_api(
    url: Option<String>,
    key: Option<String>,
    provider_type: Option<String>,
    supplier_name: Option<String>,
    sort: Option<i64>,
) -> Result<AggregateApiCreateResult, String> {
    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    let normalized_provider_type = normalize_provider_type(provider_type)?;
    let normalized_supplier_name = normalize_supplier_name(supplier_name)?;
    let normalized_sort = normalize_sort(sort);
    let normalized_url = normalize_upstream_base_url(url)?
        .unwrap_or_else(|| provider_default_url(normalized_provider_type.as_str()).to_string());
    let normalized_key = normalize_secret(key).ok_or_else(|| "key is required".to_string())?;
    let id = generate_aggregate_api_id();
    let created_at = now_ts();
    let record = AggregateApi {
        id: id.clone(),
        provider_type: normalized_provider_type,
        supplier_name: Some(normalized_supplier_name),
        sort: normalized_sort,
        url: normalized_url,
        status: "active".to_string(),
        created_at,
        updated_at: created_at,
        last_test_at: None,
        last_test_status: None,
        last_test_error: None,
    };
    storage
        .insert_aggregate_api(&record)
        .map_err(|err| err.to_string())?;
    if let Err(err) = storage.upsert_aggregate_api_secret(&id, &normalized_key) {
        let _ = storage.delete_aggregate_api(&id);
        return Err(format!("persist aggregate api secret failed: {err}"));
    }
    Ok(AggregateApiCreateResult {
        id,
        key: normalized_key,
    })
}

pub(crate) fn update_aggregate_api(
    api_id: &str,
    url: Option<String>,
    key: Option<String>,
    provider_type: Option<String>,
    supplier_name: Option<String>,
    sort: Option<i64>,
) -> Result<(), String> {
    if api_id.is_empty() {
        return Err("aggregate api id required".to_string());
    }
    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    if let Some(provider_type) = provider_type {
        let normalized_provider_type = normalize_provider_type(Some(provider_type))?;
        storage
            .update_aggregate_api_type(api_id, normalized_provider_type.as_str())
            .map_err(|err| err.to_string())?;
    }
    let normalized_supplier_name = normalize_supplier_name(supplier_name)?;
    storage
        .update_aggregate_api_supplier_name(api_id, Some(normalized_supplier_name.as_str()))
        .map_err(|err| err.to_string())?;
    if sort.is_some() {
        storage
            .update_aggregate_api_sort(api_id, normalize_sort(sort))
            .map_err(|err| err.to_string())?;
    }
    if let Some(url) = url {
        let normalized_url =
            normalize_upstream_base_url(Some(url))?.ok_or_else(|| "url is required".to_string())?;
        storage
            .update_aggregate_api(api_id, normalized_url.as_str())
            .map_err(|err| err.to_string())?;
    }
    if let Some(secret) = normalize_secret(key) {
        storage
            .upsert_aggregate_api_secret(api_id, &secret)
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

pub(crate) fn delete_aggregate_api(api_id: &str) -> Result<(), String> {
    if api_id.is_empty() {
        return Err("aggregate api id required".to_string());
    }
    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    storage
        .delete_aggregate_api(api_id)
        .map_err(|err| err.to_string())
}

pub(crate) fn read_aggregate_api_secret(api_id: &str) -> Result<AggregateApiSecretResult, String> {
    if api_id.is_empty() {
        return Err("aggregate api id required".to_string());
    }
    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    let key = storage
        .find_aggregate_api_secret_by_id(api_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "aggregate api secret not found".to_string())?;
    Ok(AggregateApiSecretResult {
        id: api_id.to_string(),
        key,
    })
}

pub(crate) fn test_aggregate_api_connection(
    api_id: &str,
) -> Result<AggregateApiTestResult, String> {
    if api_id.is_empty() {
        return Err("aggregate api id required".to_string());
    }
    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    let api = storage
        .find_aggregate_api_by_id(api_id)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| "aggregate api not found".to_string())?;
    let secret = storage
        .find_aggregate_api_secret_by_id(api_id)
        .map_err(|err| err.to_string())?;
    let Some(secret) = secret else {
        return Err("aggregate api secret not found".to_string());
    };
    let client = gateway::fresh_upstream_client();
    let started_at = Instant::now();
    let provider_type = normalize_provider_type_value(api.provider_type.as_str());
    let result = if probe_codex_only_for_provider(provider_type.as_str()) {
        probe_codex_endpoint(&client, api.url.as_str(), &secret)
    } else {
        probe_claude_endpoint(&client, api.url.as_str(), &secret)
    };
    let (ok, status_code, last_error) = match result {
        Ok(code) => (true, Some(code), None),
        Err(err) => (false, None, Some(err)),
    };
    let message = last_error.map(|err| format!("provider={provider_type}; {err}"));

    let _ = storage.update_aggregate_api_test_result(api_id, ok, status_code, message.as_deref());
    Ok(AggregateApiTestResult {
        id: api_id.to_string(),
        ok,
        status_code,
        message,
        tested_at: now_ts(),
        latency_ms: started_at.elapsed().as_millis() as i64,
    })
}
