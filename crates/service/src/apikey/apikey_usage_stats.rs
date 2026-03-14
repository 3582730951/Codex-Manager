use codexmanager_core::rpc::types::ApiKeyUsageStatSummary;

use crate::storage_helpers::open_storage;

pub(crate) fn read_api_key_usage_stats() -> Result<Vec<ApiKeyUsageStatSummary>, String> {
    let storage = open_storage().ok_or_else(|| "open storage failed".to_string())?;
    let items = storage
        .summarize_request_token_stats_by_key()
        .map_err(|err| format!("summarize api key token stats failed: {err}"))?;

    Ok(items
        .into_iter()
        .map(|item| ApiKeyUsageStatSummary {
            key_id: item.key_id,
            total_tokens: item.total_tokens.max(0),
        })
        .collect())
}
