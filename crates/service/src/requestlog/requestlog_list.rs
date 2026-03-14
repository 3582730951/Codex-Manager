use codexmanager_core::rpc::types::RequestLogSummary;

use crate::storage_helpers::open_storage;

fn sanitize_upstream_url_for_display(raw: Option<&str>) -> Option<String> {
    let trimmed = raw.map(str::trim).filter(|value| !value.is_empty())?;
    let lower = trimmed.to_ascii_lowercase();

    if lower.contains("localhost")
        || lower.contains("127.0.0.1")
        || lower.contains("0.0.0.0")
        || lower.contains("[::1]")
    {
        return Some("本地".to_string());
    }

    if lower.contains("chatgpt.com")
        || lower.contains("chat.openai.com")
        || lower.contains("api.openai.com")
        || lower.contains("/backend-api/codex")
    {
        return Some("默认".to_string());
    }

    Some("自定义".to_string())
}

pub(crate) fn read_request_logs(
    query: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<RequestLogSummary>, String> {
    let storage = open_storage().ok_or_else(|| "open storage failed".to_string())?;
    let logs = storage
        .list_request_logs(query.as_deref(), limit.unwrap_or(200))
        .map_err(|err| format!("list request logs failed: {err}"))?;
    Ok(logs
        .into_iter()
        .map(|item| RequestLogSummary {
            trace_id: item.trace_id,
            key_id: item.key_id,
            account_id: item.account_id,
            request_path: item.request_path,
            original_path: item.original_path,
            adapted_path: item.adapted_path,
            method: item.method,
            model: item.model,
            reasoning_effort: item.reasoning_effort,
            response_adapter: item.response_adapter,
            upstream_url: sanitize_upstream_url_for_display(item.upstream_url.as_deref()),
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
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::sanitize_upstream_url_for_display;

    #[test]
    fn sanitize_upstream_url_masks_official_domains() {
        assert_eq!(
            sanitize_upstream_url_for_display(Some(
                "https://chatgpt.com/backend-api/codex/responses"
            ))
            .as_deref(),
            Some("默认")
        );
        assert_eq!(
            sanitize_upstream_url_for_display(Some("https://api.openai.com/v1/responses"))
                .as_deref(),
            Some("默认")
        );
    }

    #[test]
    fn sanitize_upstream_url_masks_local_addresses() {
        assert_eq!(
            sanitize_upstream_url_for_display(Some("http://127.0.0.1:3000/relay")).as_deref(),
            Some("本地")
        );
        assert_eq!(
            sanitize_upstream_url_for_display(Some("http://localhost:3000/relay")).as_deref(),
            Some("本地")
        );
    }

    #[test]
    fn sanitize_upstream_url_masks_custom_addresses() {
        assert_eq!(
            sanitize_upstream_url_for_display(Some("https://gateway.example.com/v1")).as_deref(),
            Some("自定义")
        );
    }
}
