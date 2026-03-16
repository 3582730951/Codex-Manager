use codexmanager_core::rpc::types::{
    RequestLogListParams, RequestLogListResult, RequestLogSummary,
};
use codexmanager_core::storage::RequestLog;

use crate::storage_helpers::open_storage;

const DEFAULT_REQUEST_LOG_PAGE_SIZE: i64 = 20;
const MAX_REQUEST_LOG_PAGE_SIZE: i64 = 500;

fn normalize_upstream_url(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(crate) fn read_request_logs(
    query: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<RequestLogSummary>, String> {
    let storage = open_storage().ok_or_else(|| "open storage failed".to_string())?;
    let logs = storage
        .list_request_logs(query.as_deref(), limit.unwrap_or(200))
        .map_err(|err| format!("list request logs failed: {err}"))?;
    Ok(logs.into_iter().map(to_request_log_summary).collect())
}

pub(crate) fn read_request_log_page(
    params: RequestLogListParams,
) -> Result<RequestLogListResult, String> {
    let params = params.normalized();
    let storage = open_storage().ok_or_else(|| "open storage failed".to_string())?;
    let query = normalize_optional_text(params.query);
    let status_filter = normalize_status_filter(params.status_filter);
    let page_size = normalize_page_size(params.page_size);
    let total = storage
        .count_request_logs(query.as_deref(), status_filter.as_deref())
        .map_err(|err| format!("count request logs failed: {err}"))?;
    let page = clamp_page(params.page, total, page_size);
    let offset = (page - 1) * page_size;
    let logs = storage
        .list_request_logs_paginated(
            query.as_deref(),
            status_filter.as_deref(),
            offset,
            page_size,
        )
        .map_err(|err| format!("list request logs failed: {err}"))?;

    Ok(RequestLogListResult {
        items: logs.into_iter().map(to_request_log_summary).collect(),
        total,
        page,
        page_size,
    })
}

pub(crate) fn normalize_optional_text(value: Option<String>) -> Option<String> {
    let trimmed = value.unwrap_or_default().trim().to_string();
    if trimmed.is_empty() || trimmed == "all" {
        return None;
    }
    Some(trimmed)
}

pub(crate) fn normalize_status_filter(value: Option<String>) -> Option<String> {
    let normalized = value.unwrap_or_default().trim().to_ascii_lowercase();
    match normalized.as_str() {
        "" | "all" => None,
        "2xx" | "4xx" | "5xx" => Some(normalized),
        _ => None,
    }
}

fn normalize_page_size(value: i64) -> i64 {
    if value < 1 {
        DEFAULT_REQUEST_LOG_PAGE_SIZE
    } else {
        value.min(MAX_REQUEST_LOG_PAGE_SIZE)
    }
}

fn clamp_page(page: i64, total: i64, page_size: i64) -> i64 {
    let normalized_page = page.max(1);
    let total_pages = if total <= 0 {
        1
    } else {
        ((total + page_size - 1) / page_size).max(1)
    };
    normalized_page.min(total_pages)
}

fn to_request_log_summary(item: RequestLog) -> RequestLogSummary {
    let attempted_account_ids = item
        .attempted_account_ids_json
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
        .unwrap_or_default();
    RequestLogSummary {
        trace_id: item.trace_id,
        key_id: item.key_id,
        account_id: item.account_id,
        initial_account_id: item.initial_account_id,
        attempted_account_ids,
        request_path: item.request_path,
        original_path: item.original_path,
        adapted_path: item.adapted_path,
        method: item.method,
        model: item.model,
        reasoning_effort: item.reasoning_effort,
        response_adapter: item.response_adapter,
        upstream_url: normalize_upstream_url(item.upstream_url.as_deref()),
        status_code: item.status_code,
        duration_ms: item.duration_ms,
        input_tokens: item.input_tokens,
        cached_input_tokens: item.cached_input_tokens,
        output_tokens: item.output_tokens,
        total_tokens: item.total_tokens,
        reasoning_output_tokens: item.reasoning_output_tokens,
        estimated_cost_usd: item.estimated_cost_usd,
        error: item.error,
        created_at: item.created_at,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_optional_text, normalize_status_filter, normalize_upstream_url,
        RequestLogListParams, DEFAULT_REQUEST_LOG_PAGE_SIZE,
    };

    #[test]
    fn normalize_upstream_url_keeps_official_domains() {
        assert_eq!(
            normalize_upstream_url(Some("https://chatgpt.com/backend-api/codex/responses"))
                .as_deref(),
            Some("https://chatgpt.com/backend-api/codex/responses")
        );
        assert_eq!(
            normalize_upstream_url(Some("https://api.openai.com/v1/responses")).as_deref(),
            Some("https://api.openai.com/v1/responses")
        );
    }

    #[test]
    fn normalize_upstream_url_keeps_local_addresses() {
        assert_eq!(
            normalize_upstream_url(Some("http://127.0.0.1:3000/relay")).as_deref(),
            Some("http://127.0.0.1:3000/relay")
        );
        assert_eq!(
            normalize_upstream_url(Some("http://localhost:3000/relay")).as_deref(),
            Some("http://localhost:3000/relay")
        );
    }

    #[test]
    fn normalize_upstream_url_keeps_custom_addresses() {
        assert_eq!(
            normalize_upstream_url(Some("https://gateway.example.com/v1")).as_deref(),
            Some("https://gateway.example.com/v1")
        );
    }

    #[test]
    fn normalize_upstream_url_trims_empty_values() {
        assert_eq!(normalize_upstream_url(None), None);
        assert_eq!(normalize_upstream_url(Some("   ")), None);
        assert_eq!(
            normalize_upstream_url(Some(" https://api.openai.com/v1/responses ")).as_deref(),
            Some("https://api.openai.com/v1/responses")
        );
    }

    #[test]
    fn request_log_list_params_default_to_first_page_with_twenty_items() {
        let params: RequestLogListParams =
            serde_json::from_value(serde_json::json!({})).expect("deserialize params");
        let normalized = params.normalized();

        assert_eq!(normalized.page, 1);
        assert_eq!(normalized.page_size, DEFAULT_REQUEST_LOG_PAGE_SIZE);
    }

    #[test]
    fn normalize_status_filter_accepts_known_values() {
        assert_eq!(
            normalize_status_filter(Some("2xx".to_string())).as_deref(),
            Some("2xx")
        );
        assert_eq!(normalize_status_filter(Some("ALL".to_string())), None);
        assert_eq!(normalize_status_filter(Some("unknown".to_string())), None);
    }

    #[test]
    fn normalize_optional_text_trims_blank_values() {
        assert_eq!(normalize_optional_text(Some("  ".to_string())), None);
        assert_eq!(
            normalize_optional_text(Some(" trace:=abc ".to_string())).as_deref(),
            Some("trace:=abc")
        );
    }
}
