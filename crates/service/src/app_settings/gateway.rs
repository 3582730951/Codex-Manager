use crate::gateway;
use crate::usage_refresh;
use serde::Deserialize;

use super::{
    normalize_optional_text, save_persisted_app_setting, save_persisted_bool_setting,
    APP_SETTING_GATEWAY_AFFINITY_ROUTING_MODE_KEY,
    APP_SETTING_GATEWAY_AFFINITY_SOFT_QUOTA_PERCENT_KEY,
    APP_SETTING_GATEWAY_BACKGROUND_TASKS_KEY, APP_SETTING_GATEWAY_FREE_ACCOUNT_MAX_MODEL_KEY,
    APP_SETTING_GATEWAY_CONTEXT_REPLAY_ENABLED_KEY,
    APP_SETTING_GATEWAY_ORIGINATOR_KEY, APP_SETTING_GATEWAY_REQUEST_COMPRESSION_ENABLED_KEY,
    APP_SETTING_GATEWAY_REPLAY_MAX_TURNS_KEY,
    APP_SETTING_GATEWAY_RESIDENCY_REQUIREMENT_KEY, APP_SETTING_GATEWAY_ROUTE_STRATEGY_KEY,
    APP_SETTING_GATEWAY_SSE_KEEPALIVE_INTERVAL_MS_KEY, APP_SETTING_GATEWAY_UPSTREAM_PROXY_URL_KEY,
    APP_SETTING_GATEWAY_UPSTREAM_STREAM_TIMEOUT_MS_KEY, APP_SETTING_GATEWAY_USER_AGENT_VERSION_KEY,
};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTasksInput {
    pub usage_polling_enabled: Option<bool>,
    pub usage_poll_interval_secs: Option<u64>,
    pub gateway_keepalive_enabled: Option<bool>,
    pub gateway_keepalive_interval_secs: Option<u64>,
    pub token_refresh_polling_enabled: Option<bool>,
    pub token_refresh_poll_interval_secs: Option<u64>,
    pub usage_refresh_workers: Option<usize>,
    pub http_worker_factor: Option<usize>,
    pub http_worker_min: Option<usize>,
    pub http_stream_worker_factor: Option<usize>,
    pub http_stream_worker_min: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AffinitySettingsInput {
    pub affinity_routing_mode: Option<String>,
    pub context_replay_enabled: Option<bool>,
    pub affinity_soft_quota_percent: Option<u64>,
    pub replay_max_turns: Option<u64>,
}

impl BackgroundTasksInput {
    pub(crate) fn into_patch(self) -> usage_refresh::BackgroundTasksSettingsPatch {
        usage_refresh::BackgroundTasksSettingsPatch {
            usage_polling_enabled: self.usage_polling_enabled,
            usage_poll_interval_secs: self.usage_poll_interval_secs,
            gateway_keepalive_enabled: self.gateway_keepalive_enabled,
            gateway_keepalive_interval_secs: self.gateway_keepalive_interval_secs,
            token_refresh_polling_enabled: self.token_refresh_polling_enabled,
            token_refresh_poll_interval_secs: self.token_refresh_poll_interval_secs,
            usage_refresh_workers: self.usage_refresh_workers,
            http_worker_factor: self.http_worker_factor,
            http_worker_min: self.http_worker_min,
            http_stream_worker_factor: self.http_stream_worker_factor,
            http_stream_worker_min: self.http_stream_worker_min,
        }
    }
}

pub fn set_gateway_route_strategy(strategy: &str) -> Result<String, String> {
    let applied = gateway::set_route_strategy(strategy)?.to_string();
    save_persisted_app_setting(APP_SETTING_GATEWAY_ROUTE_STRATEGY_KEY, Some(&applied))?;
    Ok(applied)
}

pub fn set_gateway_affinity_routing_mode(mode: &str) -> Result<String, String> {
    let applied = gateway::set_affinity_routing_mode(mode)?.to_string();
    save_persisted_app_setting(APP_SETTING_GATEWAY_AFFINITY_ROUTING_MODE_KEY, Some(&applied))?;
    Ok(applied)
}

pub fn current_gateway_affinity_routing_mode() -> String {
    gateway::current_affinity_routing_mode().to_string()
}

pub fn set_gateway_context_replay_enabled(enabled: bool) -> Result<bool, String> {
    let applied = gateway::set_context_replay_enabled(enabled);
    save_persisted_bool_setting(APP_SETTING_GATEWAY_CONTEXT_REPLAY_ENABLED_KEY, applied)?;
    Ok(applied)
}

pub fn current_gateway_context_replay_enabled() -> bool {
    gateway::context_replay_enabled()
}

pub fn set_gateway_affinity_soft_quota_percent(percent: u64) -> Result<u64, String> {
    let applied = gateway::set_affinity_soft_quota_percent(percent)?;
    save_persisted_app_setting(
        APP_SETTING_GATEWAY_AFFINITY_SOFT_QUOTA_PERCENT_KEY,
        Some(&applied.to_string()),
    )?;
    Ok(applied)
}

pub fn current_gateway_affinity_soft_quota_percent() -> u64 {
    gateway::current_affinity_soft_quota_percent()
}

pub fn set_gateway_replay_max_turns(turns: u64) -> Result<u64, String> {
    let applied = gateway::set_replay_max_turns(turns)?;
    save_persisted_app_setting(
        APP_SETTING_GATEWAY_REPLAY_MAX_TURNS_KEY,
        Some(&applied.to_string()),
    )?;
    Ok(applied)
}

pub fn current_gateway_replay_max_turns() -> u64 {
    gateway::current_replay_max_turns()
}

pub fn set_gateway_affinity_settings(
    input: AffinitySettingsInput,
) -> Result<serde_json::Value, String> {
    let mode = if let Some(mode) = input.affinity_routing_mode.as_deref() {
        set_gateway_affinity_routing_mode(mode)?
    } else {
        current_gateway_affinity_routing_mode()
    };
    let context_replay_enabled = if let Some(enabled) = input.context_replay_enabled {
        set_gateway_context_replay_enabled(enabled)?
    } else {
        current_gateway_context_replay_enabled()
    };
    let affinity_soft_quota_percent = if let Some(percent) = input.affinity_soft_quota_percent {
        set_gateway_affinity_soft_quota_percent(percent)?
    } else {
        current_gateway_affinity_soft_quota_percent()
    };
    let replay_max_turns = if let Some(turns) = input.replay_max_turns {
        set_gateway_replay_max_turns(turns)?
    } else {
        current_gateway_replay_max_turns()
    };

    Ok(serde_json::json!({
        "affinityRoutingMode": mode,
        "contextReplayEnabled": context_replay_enabled,
        "affinitySoftQuotaPercent": affinity_soft_quota_percent,
        "replayMaxTurns": replay_max_turns,
    }))
}

pub fn set_gateway_free_account_max_model(model: &str) -> Result<String, String> {
    let applied = gateway::set_free_account_max_model(model)?;
    save_persisted_app_setting(
        APP_SETTING_GATEWAY_FREE_ACCOUNT_MAX_MODEL_KEY,
        Some(&applied),
    )?;
    Ok(applied)
}

pub fn current_gateway_free_account_max_model() -> String {
    gateway::current_free_account_max_model()
}

pub fn set_gateway_request_compression_enabled(enabled: bool) -> Result<bool, String> {
    let applied = gateway::set_request_compression_enabled(enabled);
    save_persisted_bool_setting(APP_SETTING_GATEWAY_REQUEST_COMPRESSION_ENABLED_KEY, applied)?;
    Ok(applied)
}

pub fn current_gateway_request_compression_enabled() -> bool {
    gateway::request_compression_enabled()
}

pub fn set_gateway_originator(originator: &str) -> Result<String, String> {
    let applied = gateway::set_originator(originator)?;
    save_persisted_app_setting(APP_SETTING_GATEWAY_ORIGINATOR_KEY, Some(&applied))?;
    Ok(applied)
}

pub fn current_gateway_originator() -> String {
    gateway::current_originator()
}

pub fn set_gateway_user_agent_version(version: &str) -> Result<String, String> {
    let applied = gateway::set_codex_user_agent_version(version)?;
    save_persisted_app_setting(APP_SETTING_GATEWAY_USER_AGENT_VERSION_KEY, Some(&applied))?;
    Ok(applied)
}

pub fn current_gateway_user_agent_version() -> String {
    gateway::current_codex_user_agent_version()
}

pub fn set_gateway_residency_requirement(value: Option<&str>) -> Result<Option<String>, String> {
    let normalized = normalize_optional_text(value);
    let applied = gateway::set_residency_requirement(normalized.as_deref())?;
    save_persisted_app_setting(
        APP_SETTING_GATEWAY_RESIDENCY_REQUIREMENT_KEY,
        applied.as_deref(),
    )?;
    Ok(applied)
}

pub fn current_gateway_residency_requirement() -> Option<String> {
    gateway::current_residency_requirement()
}

pub fn residency_requirement_options() -> &'static [&'static str] {
    &["", "us"]
}

pub fn set_gateway_upstream_proxy_url(proxy_url: Option<&str>) -> Result<Option<String>, String> {
    let normalized = normalize_optional_text(proxy_url);
    let applied = gateway::set_upstream_proxy_url(normalized.as_deref())?;
    save_persisted_app_setting(
        APP_SETTING_GATEWAY_UPSTREAM_PROXY_URL_KEY,
        applied.as_deref(),
    )?;
    Ok(applied)
}

pub fn set_gateway_upstream_stream_timeout_ms(timeout_ms: u64) -> Result<u64, String> {
    let applied = gateway::set_upstream_stream_timeout_ms(timeout_ms);
    save_persisted_app_setting(
        APP_SETTING_GATEWAY_UPSTREAM_STREAM_TIMEOUT_MS_KEY,
        Some(&applied.to_string()),
    )?;
    Ok(applied)
}

pub fn current_gateway_upstream_stream_timeout_ms() -> u64 {
    gateway::current_upstream_stream_timeout_ms()
}

pub fn set_gateway_sse_keepalive_interval_ms(interval_ms: u64) -> Result<u64, String> {
    let applied = gateway::set_sse_keepalive_interval_ms(interval_ms)?;
    save_persisted_app_setting(
        APP_SETTING_GATEWAY_SSE_KEEPALIVE_INTERVAL_MS_KEY,
        Some(&applied.to_string()),
    )?;
    Ok(applied)
}

pub fn current_gateway_sse_keepalive_interval_ms() -> u64 {
    gateway::current_sse_keepalive_interval_ms()
}

pub fn set_gateway_background_tasks(
    input: BackgroundTasksInput,
) -> Result<serde_json::Value, String> {
    let applied = usage_refresh::set_background_tasks_settings(input.into_patch());
    let raw = serde_json::to_string(&applied)
        .map_err(|err| format!("serialize background tasks failed: {err}"))?;
    save_persisted_app_setting(APP_SETTING_GATEWAY_BACKGROUND_TASKS_KEY, Some(&raw))?;
    serde_json::to_value(applied).map_err(|err| err.to_string())
}

pub(crate) fn current_background_tasks_snapshot_value() -> Result<serde_json::Value, String> {
    serde_json::to_value(usage_refresh::background_tasks_settings()).map_err(|err| err.to_string())
}
