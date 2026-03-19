use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub id: u64,
    pub result: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InitializeResult {
    pub server_name: String,
    pub version: String,
    pub user_agent: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub id: String,
    pub label: String,
    pub group_name: Option<String>,
    pub sort: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountListParams {
    pub page: i64,
    pub page_size: i64,
    pub query: Option<String>,
    pub filter: Option<String>,
    pub group_filter: Option<String>,
}

impl Default for AccountListParams {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 5,
            query: None,
            filter: None,
            group_filter: None,
        }
    }
}

impl AccountListParams {
    pub fn normalized(self) -> Self {
        // 中文注释：分页参数小于 1 时回退到默认值，避免出现负偏移或零页大小。
        Self {
            page: if self.page < 1 { 1 } else { self.page },
            page_size: if self.page_size < 1 {
                5
            } else {
                self.page_size
            },
            query: self.query,
            filter: self.filter,
            group_filter: self.group_filter,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountListResult {
    pub items: Vec<AccountSummary>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAuthInfo {
    pub user_code_url: String,
    pub token_url: String,
    pub verification_url: String,
    pub redirect_uri: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginStartResult {
    pub auth_url: String,
    pub login_id: String,
    pub login_type: String,
    pub issuer: String,
    pub client_id: String,
    pub redirect_uri: String,
    #[serde(default)]
    pub warning: Option<String>,
    pub device: Option<DeviceAuthInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshotResult {
    pub account_id: Option<String>,
    pub availability_status: Option<String>,
    pub used_percent: Option<f64>,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<i64>,
    pub secondary_used_percent: Option<f64>,
    pub secondary_window_minutes: Option<i64>,
    pub secondary_resets_at: Option<i64>,
    pub credits_json: Option<String>,
    pub captured_at: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UsageReadResult {
    pub snapshot: Option<UsageSnapshotResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitWindowResult {
    pub used_percent: i64,
    pub window_duration_mins: Option<i64>,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitSnapshotResult {
    pub limit_id: Option<String>,
    pub limit_name: Option<String>,
    pub primary: Option<RateLimitWindowResult>,
    pub secondary: Option<RateLimitWindowResult>,
    pub credits: Option<serde_json::Value>,
    pub plan_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRateLimitsReadResult {
    pub rate_limits: RateLimitSnapshotResult,
    pub rate_limits_by_limit_id:
        Option<std::collections::BTreeMap<String, RateLimitSnapshotResult>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UsageListResult {
    pub items: Vec<UsageSnapshotResult>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageAggregateSummaryResult {
    pub primary_bucket_count: i64,
    pub primary_known_count: i64,
    pub primary_unknown_count: i64,
    pub primary_remain_percent: Option<i64>,
    pub secondary_bucket_count: i64,
    pub secondary_known_count: i64,
    pub secondary_unknown_count: i64,
    pub secondary_remain_percent: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeySummary {
    pub id: String,
    pub name: Option<String>,
    pub model_slug: Option<String>,
    pub reasoning_effort: Option<String>,
    pub client_type: String,
    pub protocol_type: String,
    pub auth_scheme: String,
    pub upstream_base_url: Option<String>,
    pub static_headers_json: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiKeyListResult {
    pub items: Vec<ApiKeySummary>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyUsageStatSummary {
    pub key_id: String,
    pub total_tokens: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiKeyUsageStatListResult {
    pub items: Vec<ApiKeyUsageStatSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyCreateResult {
    pub id: String,
    pub key: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeySecretResult {
    pub id: String,
    pub key: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    pub slug: String,
    pub display_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApiKeyModelListResult {
    pub items: Vec<ModelOption>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogSummary {
    pub trace_id: Option<String>,
    pub key_id: Option<String>,
    pub account_id: Option<String>,
    pub initial_account_id: Option<String>,
    #[serde(default)]
    pub attempted_account_ids: Vec<String>,
    pub request_path: String,
    pub original_path: Option<String>,
    pub adapted_path: Option<String>,
    pub method: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub response_adapter: Option<String>,
    pub upstream_url: Option<String>,
    pub status_code: Option<i64>,
    pub duration_ms: Option<i64>,
    pub input_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub reasoning_output_tokens: Option<i64>,
    pub estimated_cost_usd: Option<f64>,
    pub error: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RequestLogListParams {
    pub page: i64,
    pub page_size: i64,
    pub query: Option<String>,
    pub status_filter: Option<String>,
}

impl Default for RequestLogListParams {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 20,
            query: None,
            status_filter: None,
        }
    }
}

impl RequestLogListParams {
    pub fn normalized(self) -> Self {
        Self {
            page: if self.page < 1 { 1 } else { self.page },
            page_size: if self.page_size < 1 {
                20
            } else {
                self.page_size
            },
            query: self.query,
            status_filter: self.status_filter,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogListResult {
    pub items: Vec<RequestLogSummary>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogFilterSummaryResult {
    pub total_count: i64,
    pub filtered_count: i64,
    pub success_count: i64,
    pub error_count: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogTodaySummaryResult {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub today_tokens: i64,
    pub estimated_cost: f64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupSnapshotResult {
    pub accounts: Vec<AccountSummary>,
    pub usage_snapshots: Vec<UsageSnapshotResult>,
    #[serde(default)]
    pub usage_aggregate_summary: UsageAggregateSummaryResult,
    pub api_keys: Vec<ApiKeySummary>,
    pub api_model_options: Vec<ModelOption>,
    pub manual_preferred_account_id: Option<String>,
    pub request_log_today_summary: RequestLogTodaySummaryResult,
    pub request_logs: Vec<RequestLogSummary>,
}

#[cfg(test)]
#[path = "tests/types_tests.rs"]
mod tests;
