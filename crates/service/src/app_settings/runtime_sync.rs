use crate::gateway;
use crate::usage_refresh;

use super::{
    apply_env_overrides_to_process, list_app_settings_map, normalize_optional_text,
    parse_bool_with_default, persisted_env_overrides_missing_process_env,
    reload_runtime_after_env_override_apply, set_service_bind_mode, BackgroundTasksInput,
    APP_SETTING_GATEWAY_AFFINITY_ROUTING_MODE_KEY,
    APP_SETTING_GATEWAY_AFFINITY_SOFT_QUOTA_PERCENT_KEY,
    APP_SETTING_GATEWAY_BACKGROUND_TASKS_KEY, APP_SETTING_GATEWAY_FREE_ACCOUNT_MAX_MODEL_KEY,
    APP_SETTING_GATEWAY_CONTEXT_REPLAY_ENABLED_KEY,
    APP_SETTING_GATEWAY_ORIGINATOR_KEY, APP_SETTING_GATEWAY_REQUEST_COMPRESSION_ENABLED_KEY,
    APP_SETTING_GATEWAY_REPLAY_MAX_TURNS_KEY,
    APP_SETTING_GATEWAY_RESIDENCY_REQUIREMENT_KEY, APP_SETTING_GATEWAY_ROUTE_STRATEGY_KEY,
    APP_SETTING_GATEWAY_SSE_KEEPALIVE_INTERVAL_MS_KEY, APP_SETTING_GATEWAY_UPSTREAM_PROXY_URL_KEY,
    APP_SETTING_GATEWAY_UPSTREAM_STREAM_TIMEOUT_MS_KEY, APP_SETTING_GATEWAY_USER_AGENT_VERSION_KEY,
    SERVICE_BIND_MODE_SETTING_KEY,
};

fn process_env_has_value(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn any_process_env_has_value(names: &[&str]) -> bool {
    names.iter().any(|name| process_env_has_value(name))
}

pub fn sync_runtime_settings_from_storage() {
    let settings = list_app_settings_map();
    let env_overrides = persisted_env_overrides_missing_process_env();
    if !env_overrides.is_empty() {
        apply_env_overrides_to_process(&env_overrides, &env_overrides);
    }
    reload_runtime_after_env_override_apply();

    if !process_env_has_value("CODEXMANAGER_SERVICE_ADDR") {
        if let Some(mode) = settings.get(SERVICE_BIND_MODE_SETTING_KEY) {
            let _ = set_service_bind_mode(mode);
        }
    }
    if !process_env_has_value("CODEXMANAGER_ROUTE_STRATEGY") {
        if let Some(strategy) = settings.get(APP_SETTING_GATEWAY_ROUTE_STRATEGY_KEY) {
            if let Some(strategy) = normalize_optional_text(Some(strategy)) {
                if let Err(err) = gateway::set_route_strategy(&strategy) {
                    log::warn!("sync persisted route strategy failed: {err}");
                }
            }
        }
    }
    if !process_env_has_value("CODEXMANAGER_AFFINITY_ROUTING_MODE") {
        if let Some(mode) = settings.get(APP_SETTING_GATEWAY_AFFINITY_ROUTING_MODE_KEY) {
            if let Some(mode) = normalize_optional_text(Some(mode)) {
                if let Err(err) = gateway::set_affinity_routing_mode(&mode) {
                    log::warn!("sync persisted affinity routing mode failed: {err}");
                }
            }
        }
    }
    if !process_env_has_value("CODEXMANAGER_CONTEXT_REPLAY_ENABLED") {
        if let Some(raw) = settings.get(APP_SETTING_GATEWAY_CONTEXT_REPLAY_ENABLED_KEY) {
            gateway::set_context_replay_enabled(parse_bool_with_default(raw, true));
        }
    }
    if !process_env_has_value("CODEXMANAGER_AFFINITY_SOFT_QUOTA_PERCENT") {
        if let Some(raw) = settings.get(APP_SETTING_GATEWAY_AFFINITY_SOFT_QUOTA_PERCENT_KEY) {
            if let Ok(percent) = raw.trim().parse::<u64>() {
                if let Err(err) = gateway::set_affinity_soft_quota_percent(percent) {
                    log::warn!("sync persisted affinity soft quota percent failed: {err}");
                }
            } else {
                log::warn!("parse persisted affinity soft quota percent failed: {raw}");
            }
        }
    }
    if !process_env_has_value("CODEXMANAGER_REPLAY_MAX_TURNS") {
        if let Some(raw) = settings.get(APP_SETTING_GATEWAY_REPLAY_MAX_TURNS_KEY) {
            if let Ok(turns) = raw.trim().parse::<u64>() {
                if let Err(err) = gateway::set_replay_max_turns(turns) {
                    log::warn!("sync persisted replay max turns failed: {err}");
                }
            } else {
                log::warn!("parse persisted replay max turns failed: {raw}");
            }
        }
    }
    if !process_env_has_value("CODEXMANAGER_FREE_ACCOUNT_MAX_MODEL") {
        if let Some(model) = settings.get(APP_SETTING_GATEWAY_FREE_ACCOUNT_MAX_MODEL_KEY) {
            if let Some(model) = normalize_optional_text(Some(model)) {
                if let Err(err) = gateway::set_free_account_max_model(&model) {
                    log::warn!("sync persisted free account max model failed: {err}");
                }
            }
        }
    }
    if !process_env_has_value("CODEXMANAGER_ENABLE_REQUEST_COMPRESSION") {
        if let Some(raw) = settings.get(APP_SETTING_GATEWAY_REQUEST_COMPRESSION_ENABLED_KEY) {
            gateway::set_request_compression_enabled(parse_bool_with_default(raw, true));
        }
    }
    if !process_env_has_value("CODEXMANAGER_ORIGINATOR") {
        if let Some(originator) = settings.get(APP_SETTING_GATEWAY_ORIGINATOR_KEY) {
            if let Some(originator) = normalize_optional_text(Some(originator)) {
                if let Err(err) = gateway::set_originator(&originator) {
                    log::warn!("sync persisted gateway originator failed: {err}");
                }
            }
        }
    }
    if let Some(version) = settings.get(APP_SETTING_GATEWAY_USER_AGENT_VERSION_KEY) {
        if let Some(version) = normalize_optional_text(Some(version)) {
            if let Err(err) = gateway::set_codex_user_agent_version(&version) {
                log::warn!("sync persisted gateway user agent version failed: {err}");
            }
        }
    }
    if !process_env_has_value("CODEXMANAGER_RESIDENCY_REQUIREMENT") {
        if let Some(residency_requirement) =
            settings.get(APP_SETTING_GATEWAY_RESIDENCY_REQUIREMENT_KEY)
        {
            let normalized = normalize_optional_text(Some(residency_requirement));
            if let Err(err) = gateway::set_residency_requirement(normalized.as_deref()) {
                log::warn!("sync persisted gateway residency requirement failed: {err}");
            }
        }
    }
    if !process_env_has_value("CODEXMANAGER_UPSTREAM_PROXY_URL") {
        if let Some(proxy_url) = settings.get(APP_SETTING_GATEWAY_UPSTREAM_PROXY_URL_KEY) {
            let normalized = normalize_optional_text(Some(proxy_url));
            if let Err(err) = gateway::set_upstream_proxy_url(normalized.as_deref()) {
                log::warn!("sync persisted upstream proxy failed: {err}");
            }
        }
    }
    if !process_env_has_value("CODEXMANAGER_UPSTREAM_STREAM_TIMEOUT_MS") {
        if let Some(raw) = settings.get(APP_SETTING_GATEWAY_UPSTREAM_STREAM_TIMEOUT_MS_KEY) {
            if let Ok(timeout_ms) = raw.trim().parse::<u64>() {
                gateway::set_upstream_stream_timeout_ms(timeout_ms);
            } else {
                log::warn!("parse persisted upstream stream timeout failed: {raw}");
            }
        }
    }
    if !process_env_has_value("CODEXMANAGER_SSE_KEEPALIVE_INTERVAL_MS") {
        if let Some(raw) = settings.get(APP_SETTING_GATEWAY_SSE_KEEPALIVE_INTERVAL_MS_KEY) {
            if let Ok(interval_ms) = raw.trim().parse::<u64>() {
                if let Err(err) = gateway::set_sse_keepalive_interval_ms(interval_ms) {
                    log::warn!("sync persisted sse keepalive interval failed: {err}");
                }
            } else {
                log::warn!("parse persisted sse keepalive interval failed: {raw}");
            }
        }
    }
    if !any_process_env_has_value(&[
        "CODEXMANAGER_USAGE_POLLING_ENABLED",
        "CODEXMANAGER_USAGE_POLL_INTERVAL_SECS",
        "CODEXMANAGER_GATEWAY_KEEPALIVE_ENABLED",
        "CODEXMANAGER_GATEWAY_KEEPALIVE_INTERVAL_SECS",
        "CODEXMANAGER_TOKEN_REFRESH_POLLING_ENABLED",
        "CODEXMANAGER_TOKEN_REFRESH_POLL_INTERVAL_SECS",
        "CODEXMANAGER_HTTP_WORKER_FACTOR",
        "CODEXMANAGER_HTTP_WORKER_MIN",
        "CODEXMANAGER_HTTP_STREAM_WORKER_FACTOR",
        "CODEXMANAGER_HTTP_STREAM_WORKER_MIN",
    ]) {
        if let Some(raw) = settings.get(APP_SETTING_GATEWAY_BACKGROUND_TASKS_KEY) {
            match serde_json::from_str::<BackgroundTasksInput>(raw) {
                Ok(input) => {
                    usage_refresh::set_background_tasks_settings(input.into_patch());
                }
                Err(err) => {
                    log::warn!("parse persisted background tasks failed: {err}");
                }
            }
        }
    }
}
