use codexmanager_core::rpc::types::{JsonRpcRequest, JsonRpcResponse};
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_SKILL_FILE_NAME: &str = "SKILL.md";
const DEFAULT_SKILL_CONFIG_FILE_NAME: &str = "codexmanager.skills-config.json";
const CONFIG_WRITE_ERROR_INVALID_REQUEST_CODE: &str = "invalid_request_payload";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigReadParams {
    #[serde(default)]
    include_layers: bool,
    #[allow(dead_code)]
    cwd: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigValueWriteParams {
    key_path: String,
    value: Value,
    merge_strategy: MergeStrategy,
    file_path: Option<String>,
    expected_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigBatchWriteParams {
    edits: Vec<ConfigEdit>,
    file_path: Option<String>,
    expected_version: Option<String>,
    #[serde(default)]
    reload_user_config: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigEdit {
    key_path: String,
    value: Value,
    merge_strategy: MergeStrategy,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum MergeStrategy {
    Replace,
    Upsert,
}

#[derive(Debug, Clone)]
struct ConfigWriteFailure {
    message: String,
    config_write_error_code: &'static str,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelListParams {
    cursor: Option<String>,
    limit: Option<u32>,
    #[allow(dead_code)]
    include_hidden: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExternalAgentConfigDetectParams {
    #[allow(dead_code)]
    include_home: bool,
    #[allow(dead_code)]
    cwds: Option<Vec<PathBuf>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillsConfigWriteParams {
    path: PathBuf,
    enabled: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillsRemoteReadParams {
    #[allow(dead_code)]
    hazelnut_scope: Option<Value>,
    #[allow(dead_code)]
    product_surface: Option<Value>,
    #[allow(dead_code)]
    enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillsRemoteWriteParams {
    hazelnut_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginInstallParams {
    marketplace_path: PathBuf,
    plugin_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginUninstallParams {
    plugin_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpServerOauthLoginParams {
    name: String,
    #[allow(dead_code)]
    scopes: Option<Vec<String>>,
    #[allow(dead_code)]
    timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewStartParams {
    thread_id: String,
    #[allow(dead_code)]
    target: Value,
}

pub(super) fn try_handle(req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
    let result = match req.method.as_str() {
        "initialized" => super::ok_result(),
        "experimentalFeature/list" => experimental_feature_list(req),
        "collaborationMode/list" => collaboration_mode_list(),
        "skills/list" => super::value_or_error(skills_list(req.params.as_ref())),
        "skills/config/write" => super::value_or_error(skills_config_write(req.params.as_ref())),
        "skills/remote/list" => super::value_or_error(skills_remote_list(req.params.as_ref())),
        "skills/remote/export" => super::value_or_error(skills_remote_export(req.params.as_ref())),
        "plugin/list" => serde_json::json!({
            "marketplaces": [],
            "remoteSyncError": Value::Null
        }),
        "plugin/install" => super::value_or_error(plugin_install(req.params.as_ref())),
        "plugin/uninstall" => super::value_or_error(plugin_uninstall(req.params.as_ref())),
        "app/list" => serde_json::json!({
            "data": [],
            "nextCursor": Value::Null
        }),
        "model/list" => super::value_or_error(model_list(req.params.as_ref())),
        "config/read" => super::value_or_error(config_read(req.params.as_ref())),
        "config/value/write" => config_write_result(config_value_write(req.params.as_ref())),
        "config/batchWrite" => config_write_result(config_batch_write(req.params.as_ref())),
        "configRequirements/read" => serde_json::json!({
            "requirements": Value::Null
        }),
        "config/mcpServer/reload" => serde_json::json!({}),
        "mcpServerStatus/list" => serde_json::json!({
            "data": [],
            "nextCursor": Value::Null
        }),
        "mcpServer/oauth/login" => {
            super::value_or_error(mcp_server_oauth_login(req.params.as_ref()))
        }
        "externalAgentConfig/detect" => {
            super::value_or_error(external_agent_config_detect(req.params.as_ref()))
        }
        "externalAgentConfig/import" => serde_json::json!({}),
        "review/start" => super::value_or_error(review_start(req.params.as_ref())),
        _ => return None,
    };

    Some(super::response(req, result))
}

fn config_write_result(result: Result<Value, ConfigWriteFailure>) -> Value {
    match result {
        Ok(value) => value,
        Err(err) => serde_json::json!({
            "error": err.message,
            "errorCode": CONFIG_WRITE_ERROR_INVALID_REQUEST_CODE,
            "configWriteErrorCode": err.config_write_error_code,
            "errorDetail": {
                "code": CONFIG_WRITE_ERROR_INVALID_REQUEST_CODE,
                "message": err.message,
                "configWriteErrorCode": err.config_write_error_code,
            }
        }),
    }
}

fn experimental_feature_list(req: &JsonRpcRequest) -> Value {
    let limit = super::i64_param(req, "limit")
        .filter(|value| *value > 0)
        .map(|value| value as usize)
        .unwrap_or(50);

    let items = vec![
        serde_json::json!({
            "name": "requestCompression",
            "stage": "stable",
            "displayName": Value::Null,
            "description": Value::Null,
            "announcement": Value::Null,
            "enabled": crate::gateway::request_compression_enabled(),
            "defaultEnabled": true
        }),
        serde_json::json!({
            "name": "responsesWebsocketTransport",
            "stage": "underDevelopment",
            "displayName": "Responses WebSocket Transport",
            "description": "CodexManager 正在补齐官方 Responses WebSocket transport / prewarm / reuse。",
            "announcement": Value::Null,
            "enabled": false,
            "defaultEnabled": false
        }),
        serde_json::json!({
            "name": "collaborationModes",
            "stage": "beta",
            "displayName": "Collaboration Modes",
            "description": "提供和官方 Codex 一致的模式列表接口，当前仅暴露内置基础模式。",
            "announcement": Value::Null,
            "enabled": true,
            "defaultEnabled": true
        }),
    ];

    serde_json::json!({
        "data": items.into_iter().take(limit).collect::<Vec<_>>(),
        "nextCursor": Value::Null
    })
}

fn collaboration_mode_list() -> Value {
    serde_json::json!({
        "data": [
            {
                "name": "Plan",
                "mode": "plan",
                "model": Value::Null,
                "reasoningEffort": "medium"
            },
            {
                "name": "Default",
                "mode": "default",
                "model": Value::Null,
                "reasoningEffort": Value::Null
            }
        ]
    })
}

fn model_list(params: Option<&Value>) -> Result<Value, String> {
    let params = parse_model_list_params(params)?;
    let mut items = crate::apikey_models::read_model_options(false)
        .map(|result| result.items)
        .unwrap_or_default();
    if items.is_empty() {
        items = crate::app_settings_get()
            .ok()
            .and_then(|snapshot| snapshot.get("freeAccountMaxModelOptions").cloned())
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .map(|slug| codexmanager_core::rpc::types::ModelOption {
                display_name: slug.clone(),
                slug,
            })
            .collect();
    }

    let limit = params.limit.unwrap_or(50).max(1) as usize;
    let offset = match params.cursor {
        Some(cursor) if !cursor.trim().is_empty() => cursor
            .parse::<usize>()
            .map_err(|_| format!("invalid cursor: {cursor}"))?,
        _ => 0,
    };
    let total = items.len();
    let page = items
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let next_cursor = if offset + page.len() < total {
        Some((offset + limit).to_string())
    } else {
        None
    };

    let data = page
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let slug = item.slug;
            let display_name = item.display_name;
            serde_json::json!({
                "id": slug.clone(),
                "model": slug,
                "upgrade": Value::Null,
                "upgradeInfo": Value::Null,
                "availabilityNux": Value::Null,
                "displayName": display_name,
                "description": "",
                "hidden": false,
                "supportedReasoningEfforts": [
                    {
                        "reasoningEffort": "medium",
                        "description": "Balanced default reasoning"
                    }
                ],
                "defaultReasoningEffort": "medium",
                "inputModalities": ["text"],
                "supportsPersonality": false,
                "isDefault": offset == 0 && index == 0,
            })
        })
        .collect::<Vec<_>>();

    Ok(serde_json::json!({
        "data": data,
        "nextCursor": next_cursor
    }))
}

fn parse_model_list_params(params: Option<&Value>) -> Result<ModelListParams, String> {
    match params {
        Some(value) => serde_json::from_value::<ModelListParams>(value.clone())
            .map_err(|err| format!("invalid model/list params: {err}")),
        None => Ok(ModelListParams::default()),
    }
}

fn external_agent_config_detect(params: Option<&Value>) -> Result<Value, String> {
    let _ = match params {
        Some(value) => serde_json::from_value::<ExternalAgentConfigDetectParams>(value.clone())
            .map_err(|err| format!("invalid externalAgentConfig/detect params: {err}"))?,
        None => ExternalAgentConfigDetectParams::default(),
    };
    Ok(serde_json::json!({
        "items": []
    }))
}

fn skills_remote_list(params: Option<&Value>) -> Result<Value, String> {
    let _ = match params {
        Some(value) => serde_json::from_value::<SkillsRemoteReadParams>(value.clone())
            .map_err(|err| format!("invalid skills/remote/list params: {err}"))?,
        None => SkillsRemoteReadParams::default(),
    };
    Ok(serde_json::json!({
        "data": []
    }))
}

fn skills_remote_export(params: Option<&Value>) -> Result<Value, String> {
    let value = params.ok_or_else(|| "skills/remote/export params are required".to_string())?;
    let params = serde_json::from_value::<SkillsRemoteWriteParams>(value.clone())
        .map_err(|err| format!("invalid skills/remote/export params: {err}"))?;
    Err(format!(
        "remote skill export is not available in CodexManager yet: {}",
        params.hazelnut_id
    ))
}

fn plugin_install(params: Option<&Value>) -> Result<Value, String> {
    let value = params.ok_or_else(|| "plugin/install params are required".to_string())?;
    let params = serde_json::from_value::<PluginInstallParams>(value.clone())
        .map_err(|err| format!("invalid plugin/install params: {err}"))?;
    Err(format!(
        "plugin install is not available in CodexManager yet: {} from {}",
        params.plugin_name,
        params.marketplace_path.display()
    ))
}

fn plugin_uninstall(params: Option<&Value>) -> Result<Value, String> {
    let value = params.ok_or_else(|| "plugin/uninstall params are required".to_string())?;
    let params = serde_json::from_value::<PluginUninstallParams>(value.clone())
        .map_err(|err| format!("invalid plugin/uninstall params: {err}"))?;
    Err(format!(
        "plugin uninstall is not available in CodexManager yet: {}",
        params.plugin_id
    ))
}

fn mcp_server_oauth_login(params: Option<&Value>) -> Result<Value, String> {
    let value = params.ok_or_else(|| "mcpServer/oauth/login params are required".to_string())?;
    let params = serde_json::from_value::<McpServerOauthLoginParams>(value.clone())
        .map_err(|err| format!("invalid mcpServer/oauth/login params: {err}"))?;
    Err(format!(
        "mcpServer oauth login is not available in CodexManager yet: {}",
        params.name
    ))
}

fn review_start(params: Option<&Value>) -> Result<Value, String> {
    let value = params.ok_or_else(|| "review/start params are required".to_string())?;
    let params = serde_json::from_value::<ReviewStartParams>(value.clone())
        .map_err(|err| format!("invalid review/start params: {err}"))?;
    Err(format!(
        "review/start is not available until thread/turn lifecycle is implemented: {}",
        params.thread_id
    ))
}

fn config_read(params: Option<&Value>) -> Result<Value, String> {
    let params = parse_config_read_params(params)?;
    let snapshot = crate::app_settings_get()?;
    let config = build_compat_config(&snapshot)?;
    sync_compat_config_file(&config)?;
    let file_path = compat_config_file_path()?;
    let version = config_version(&config)?;
    let origins = build_config_origins(&config, &file_path, &version);

    let mut result = Map::new();
    result.insert("config".to_string(), config.clone());
    result.insert("origins".to_string(), Value::Object(origins));
    if params.include_layers {
        result.insert(
            "layers".to_string(),
            serde_json::json!([
                {
                    "name": {
                        "type": "user",
                        "file": file_path,
                    },
                    "version": version,
                    "config": config,
                }
            ]),
        );
    }

    Ok(Value::Object(result))
}

fn config_value_write(params: Option<&Value>) -> Result<Value, ConfigWriteFailure> {
    let params = parse_config_value_write_params(params)
        .map_err(|message| config_write_failure("configValidationError", message))?;
    let mut config = current_compat_config()?;
    let file_path = compat_config_file_path_for_write(params.file_path.as_deref())?;
    let current_version = config_version(&config)
        .map_err(|message| config_write_failure("configValidationError", message))?;
    ensure_expected_version(params.expected_version.as_deref(), current_version.as_str())?;
    apply_config_edit(
        &mut config,
        params.key_path.as_str(),
        params.value,
        params.merge_strategy,
    )?;
    persist_compat_config(&config)?;
    config_write_response(&config, file_path)
}

fn config_batch_write(params: Option<&Value>) -> Result<Value, ConfigWriteFailure> {
    let params = parse_config_batch_write_params(params)
        .map_err(|message| config_write_failure("configValidationError", message))?;
    let mut config = current_compat_config()?;
    let file_path = compat_config_file_path_for_write(params.file_path.as_deref())?;
    let current_version = config_version(&config)
        .map_err(|message| config_write_failure("configValidationError", message))?;
    ensure_expected_version(params.expected_version.as_deref(), current_version.as_str())?;

    for edit in params.edits {
        apply_config_edit(
            &mut config,
            edit.key_path.as_str(),
            edit.value,
            edit.merge_strategy,
        )?;
    }

    let _reload_user_config = params.reload_user_config;
    persist_compat_config(&config)?;
    config_write_response(&config, file_path)
}

fn parse_config_read_params(params: Option<&Value>) -> Result<ConfigReadParams, String> {
    match params {
        Some(value) => serde_json::from_value::<ConfigReadParams>(value.clone())
            .map_err(|err| format!("invalid config/read params: {err}")),
        None => Ok(ConfigReadParams {
            include_layers: false,
            cwd: None,
        }),
    }
}

fn parse_config_value_write_params(
    params: Option<&Value>,
) -> Result<ConfigValueWriteParams, String> {
    let value = params.ok_or_else(|| "config/value/write params are required".to_string())?;
    serde_json::from_value::<ConfigValueWriteParams>(value.clone())
        .map_err(|err| format!("invalid config/value/write params: {err}"))
}

fn parse_config_batch_write_params(
    params: Option<&Value>,
) -> Result<ConfigBatchWriteParams, String> {
    let value = params.ok_or_else(|| "config/batchWrite params are required".to_string())?;
    serde_json::from_value::<ConfigBatchWriteParams>(value.clone())
        .map_err(|err| format!("invalid config/batchWrite params: {err}"))
}

fn current_compat_config() -> Result<Value, ConfigWriteFailure> {
    let snapshot = crate::app_settings_get()
        .map_err(|message| config_write_failure("configValidationError", message))?;
    build_compat_config(&snapshot)
        .map_err(|message| config_write_failure("configValidationError", message))
}

fn build_compat_config(snapshot: &Value) -> Result<Value, String> {
    let background_tasks = snapshot
        .get("backgroundTasks")
        .map(background_tasks_config_from_snapshot)
        .unwrap_or_else(|| serde_json::json!({}));

    let config = serde_json::json!({
        "app": {
            "update_auto_check": required_bool(snapshot, "updateAutoCheck")?,
            "close_to_tray_on_close": required_bool(snapshot, "closeToTrayOnClose")?,
            "lightweight_mode_on_close_to_tray": required_bool(snapshot, "lightweightModeOnCloseToTray")?,
        },
        "ui": {
            "low_transparency": required_bool(snapshot, "lowTransparency")?,
            "theme": required_string(snapshot, "theme")?,
        },
        "service": {
            "addr": optional_string(snapshot, "serviceAddr"),
            "bind_mode": required_string(snapshot, "serviceListenMode")?,
        },
        "gateway": {
            "route_strategy": required_string(snapshot, "routeStrategy")?,
            "free_account_max_model": required_string(snapshot, "freeAccountMaxModel")?,
            "request_compression_enabled": required_bool(snapshot, "requestCompressionEnabled")?,
            "originator": required_string(snapshot, "gatewayOriginator")?,
            "residency_requirement": optional_string(snapshot, "gatewayResidencyRequirement"),
            "header_policy": {
                "no_cookie_mode": required_bool(snapshot, "cpaNoCookieHeaderModeEnabled")?,
            },
            "upstream_proxy_url": optional_string(snapshot, "upstreamProxyUrl"),
            "upstream_stream_timeout_ms": required_u64(snapshot, "upstreamStreamTimeoutMs")?,
            "sse_keepalive_interval_ms": required_u64(snapshot, "sseKeepaliveIntervalMs")?,
            "background_tasks": background_tasks,
        }
    });

    Ok(config)
}

fn build_config_origins(config: &Value, file_path: &Path, version: &str) -> Map<String, Value> {
    let mut origins = Map::new();
    let file = file_path.to_string_lossy().to_string();
    collect_config_leaf_paths(config, None, &mut |key_path| {
        origins.insert(
            key_path.to_string(),
            serde_json::json!({
                "name": {
                    "type": "user",
                    "file": file,
                },
                "version": version,
            }),
        );
    });
    origins
}

fn collect_config_leaf_paths(value: &Value, prefix: Option<String>, push: &mut impl FnMut(&str)) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let next = match &prefix {
                    Some(prefix) => format!("{prefix}.{key}"),
                    None => key.clone(),
                };
                collect_config_leaf_paths(child, Some(next), push);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let next = match &prefix {
                    Some(prefix) => format!("{prefix}.{index}"),
                    None => index.to_string(),
                };
                collect_config_leaf_paths(child, Some(next), push);
            }
        }
        _ => {
            if let Some(prefix) = prefix {
                push(prefix.as_str());
            }
        }
    }
}

fn compat_config_file_path_for_write(
    requested: Option<&str>,
) -> Result<PathBuf, ConfigWriteFailure> {
    let path = compat_config_file_path()
        .map_err(|message| config_write_failure("configValidationError", message))?;
    if let Some(requested) = requested {
        let requested = requested.trim();
        if !requested.is_empty() && PathBuf::from(requested) != path {
            return Err(config_write_failure(
                "userLayerNotFound",
                format!(
                    "unsupported config file path: {requested}; only {} is writable",
                    path.display()
                ),
            ));
        }
    }
    Ok(path)
}

fn compat_config_file_path() -> Result<PathBuf, String> {
    crate::initialize_storage_if_needed()?;
    let db_path = std::env::var("CODEXMANAGER_DB_PATH")
        .map(PathBuf::from)
        .map_err(|_| "CODEXMANAGER_DB_PATH is not configured".to_string())?;
    let parent = db_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(parent.join("codexmanager.compat-config.json"))
}

fn sync_compat_config_file(config: &Value) -> Result<(), String> {
    let path = compat_config_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "create compat config dir {} failed: {err}",
                parent.display()
            )
        })?;
    }
    let raw = serde_json::to_vec_pretty(config)
        .map_err(|err| format!("serialize compat config failed: {err}"))?;
    fs::write(&path, raw)
        .map_err(|err| format!("write compat config {} failed: {err}", path.display()))
}

fn config_version(config: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(config)
        .map_err(|err| format!("serialize config version fingerprint failed: {err}"))?;
    let digest = Sha256::digest(bytes);
    Ok(format!("sha256:{digest:x}"))
}

fn ensure_expected_version(
    expected_version: Option<&str>,
    current_version: &str,
) -> Result<(), ConfigWriteFailure> {
    let Some(expected_version) = expected_version
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    if expected_version != current_version {
        return Err(config_write_failure(
            "configVersionConflict",
            format!(
                "config version conflict: expected {expected_version}, current {current_version}"
            ),
        ));
    }
    Ok(())
}

fn apply_config_edit(
    config: &mut Value,
    key_path: &str,
    value: Value,
    merge_strategy: MergeStrategy,
) -> Result<(), ConfigWriteFailure> {
    if !is_supported_config_key_path(key_path) {
        return Err(config_write_failure(
            "configSchemaUnknownKey",
            format!("unsupported config key path: {key_path}"),
        ));
    }
    if value.is_null() {
        return Err(config_write_failure(
            "configValidationError",
            format!("null is not supported for config key path: {key_path}"),
        ));
    }
    let segments = key_path
        .split('.')
        .filter(|segment| !segment.trim().is_empty())
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return Err(config_write_failure(
            "configPathNotFound",
            "config key path cannot be empty".to_string(),
        ));
    }
    set_nested_value(config, &segments, value, merge_strategy)
}

fn set_nested_value(
    current: &mut Value,
    segments: &[&str],
    value: Value,
    merge_strategy: MergeStrategy,
) -> Result<(), ConfigWriteFailure> {
    if segments.is_empty() {
        *current = value;
        return Ok(());
    }
    let Value::Object(map) = current else {
        return Err(config_write_failure(
            "configPathNotFound",
            format!(
                "config path parent is not an object: {}",
                segments.join(".")
            ),
        ));
    };
    if segments.len() == 1 {
        let key = segments[0].to_string();
        match merge_strategy {
            MergeStrategy::Replace => {
                map.insert(key, value);
            }
            MergeStrategy::Upsert => match map.get_mut(&key) {
                Some(existing) => merge_json_values(existing, value),
                None => {
                    map.insert(key, value);
                }
            },
        }
        return Ok(());
    }

    let entry = map
        .entry(segments[0].to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    set_nested_value(entry, &segments[1..], value, merge_strategy)
}

fn merge_json_values(existing: &mut Value, incoming: Value) {
    match (existing, incoming) {
        (Value::Object(existing_obj), Value::Object(incoming_obj)) => {
            for (key, incoming_value) in incoming_obj {
                match existing_obj.get_mut(&key) {
                    Some(existing_value) => merge_json_values(existing_value, incoming_value),
                    None => {
                        existing_obj.insert(key, incoming_value);
                    }
                }
            }
        }
        (slot, incoming) => {
            *slot = incoming;
        }
    }
}

fn is_supported_config_key_path(key_path: &str) -> bool {
    matches!(
        key_path,
        "app.update_auto_check"
            | "app.close_to_tray_on_close"
            | "app.lightweight_mode_on_close_to_tray"
            | "ui.low_transparency"
            | "ui.theme"
            | "service.addr"
            | "service.bind_mode"
            | "gateway.route_strategy"
            | "gateway.free_account_max_model"
            | "gateway.request_compression_enabled"
            | "gateway.originator"
            | "gateway.residency_requirement"
            | "gateway.header_policy.no_cookie_mode"
            | "gateway.upstream_proxy_url"
            | "gateway.upstream_stream_timeout_ms"
            | "gateway.sse_keepalive_interval_ms"
            | "gateway.background_tasks"
    ) || key_path.starts_with("gateway.background_tasks.")
}

fn persist_compat_config(config: &Value) -> Result<(), ConfigWriteFailure> {
    let patch = compat_config_to_app_settings_patch(config)?;
    crate::app_settings_set(Some(&patch))
        .map_err(|message| config_write_failure("configValidationError", message))?;
    sync_compat_config_file(config)
        .map_err(|message| config_write_failure("configValidationError", message))
}

fn compat_config_to_app_settings_patch(config: &Value) -> Result<Value, ConfigWriteFailure> {
    let mut patch = Map::new();
    patch.insert(
        "updateAutoCheck".to_string(),
        Value::Bool(required_bool_path(config, &["app", "update_auto_check"])?),
    );
    patch.insert(
        "closeToTrayOnClose".to_string(),
        Value::Bool(required_bool_path(
            config,
            &["app", "close_to_tray_on_close"],
        )?),
    );
    patch.insert(
        "lightweightModeOnCloseToTray".to_string(),
        Value::Bool(required_bool_path(
            config,
            &["app", "lightweight_mode_on_close_to_tray"],
        )?),
    );
    patch.insert(
        "lowTransparency".to_string(),
        Value::Bool(required_bool_path(config, &["ui", "low_transparency"])?),
    );
    patch.insert(
        "theme".to_string(),
        Value::String(required_string_path(config, &["ui", "theme"])?),
    );
    patch.insert(
        "serviceAddr".to_string(),
        optional_string_path(config, &["service", "addr"])
            .map(Value::String)
            .unwrap_or(Value::String(String::new())),
    );
    patch.insert(
        "serviceListenMode".to_string(),
        Value::String(required_string_path(config, &["service", "bind_mode"])?),
    );
    patch.insert(
        "routeStrategy".to_string(),
        Value::String(required_string_path(
            config,
            &["gateway", "route_strategy"],
        )?),
    );
    patch.insert(
        "freeAccountMaxModel".to_string(),
        Value::String(required_string_path(
            config,
            &["gateway", "free_account_max_model"],
        )?),
    );
    patch.insert(
        "requestCompressionEnabled".to_string(),
        Value::Bool(required_bool_path(
            config,
            &["gateway", "request_compression_enabled"],
        )?),
    );
    patch.insert(
        "gatewayOriginator".to_string(),
        Value::String(required_string_path(config, &["gateway", "originator"])?),
    );
    patch.insert(
        "gatewayResidencyRequirement".to_string(),
        optional_string_path(config, &["gateway", "residency_requirement"])
            .map(Value::String)
            .unwrap_or(Value::String(String::new())),
    );
    patch.insert(
        "cpaNoCookieHeaderModeEnabled".to_string(),
        Value::Bool(required_bool_path(
            config,
            &["gateway", "header_policy", "no_cookie_mode"],
        )?),
    );
    patch.insert(
        "upstreamProxyUrl".to_string(),
        optional_string_path(config, &["gateway", "upstream_proxy_url"])
            .map(Value::String)
            .unwrap_or(Value::String(String::new())),
    );
    patch.insert(
        "upstreamStreamTimeoutMs".to_string(),
        Value::Number(
            required_u64_path(config, &["gateway", "upstream_stream_timeout_ms"])?.into(),
        ),
    );
    patch.insert(
        "sseKeepaliveIntervalMs".to_string(),
        Value::Number(required_u64_path(config, &["gateway", "sse_keepalive_interval_ms"])?.into()),
    );
    patch.insert(
        "backgroundTasks".to_string(),
        background_tasks_patch_value_from_config(required_object_path(
            config,
            &["gateway", "background_tasks"],
        )?),
    );
    Ok(Value::Object(patch))
}

fn background_tasks_config_from_snapshot(value: &Value) -> Value {
    serde_json::json!({
        "usage_polling_enabled": value.get("usagePollingEnabled").and_then(Value::as_bool).unwrap_or(true),
        "usage_poll_interval_secs": value.get("usagePollIntervalSecs").and_then(Value::as_u64).unwrap_or(600),
        "gateway_keepalive_enabled": value.get("gatewayKeepaliveEnabled").and_then(Value::as_bool).unwrap_or(true),
        "gateway_keepalive_interval_secs": value.get("gatewayKeepaliveIntervalSecs").and_then(Value::as_u64).unwrap_or(180),
        "token_refresh_polling_enabled": value.get("tokenRefreshPollingEnabled").and_then(Value::as_bool).unwrap_or(true),
        "token_refresh_poll_interval_secs": value.get("tokenRefreshPollIntervalSecs").and_then(Value::as_u64).unwrap_or(60),
        "usage_refresh_workers": value.get("usageRefreshWorkers").and_then(Value::as_u64).unwrap_or(4),
        "http_worker_factor": value.get("httpWorkerFactor").and_then(Value::as_u64).unwrap_or(4),
        "http_worker_min": value.get("httpWorkerMin").and_then(Value::as_u64).unwrap_or(8),
        "http_stream_worker_factor": value.get("httpStreamWorkerFactor").and_then(Value::as_u64).unwrap_or(2),
        "http_stream_worker_min": value.get("httpStreamWorkerMin").and_then(Value::as_u64).unwrap_or(2),
        "requires_restart_keys": value.get("requiresRestartKeys").cloned().unwrap_or_else(|| serde_json::json!([])),
    })
}

fn background_tasks_patch_value_from_config(value: Value) -> Value {
    serde_json::json!({
        "usagePollingEnabled": value.get("usage_polling_enabled").and_then(Value::as_bool).unwrap_or(true),
        "usagePollIntervalSecs": value.get("usage_poll_interval_secs").and_then(Value::as_u64).unwrap_or(600),
        "gatewayKeepaliveEnabled": value.get("gateway_keepalive_enabled").and_then(Value::as_bool).unwrap_or(true),
        "gatewayKeepaliveIntervalSecs": value.get("gateway_keepalive_interval_secs").and_then(Value::as_u64).unwrap_or(180),
        "tokenRefreshPollingEnabled": value.get("token_refresh_polling_enabled").and_then(Value::as_bool).unwrap_or(true),
        "tokenRefreshPollIntervalSecs": value.get("token_refresh_poll_interval_secs").and_then(Value::as_u64).unwrap_or(60),
        "usageRefreshWorkers": value.get("usage_refresh_workers").and_then(Value::as_u64).unwrap_or(4),
        "httpWorkerFactor": value.get("http_worker_factor").and_then(Value::as_u64).unwrap_or(4),
        "httpWorkerMin": value.get("http_worker_min").and_then(Value::as_u64).unwrap_or(8),
        "httpStreamWorkerFactor": value.get("http_stream_worker_factor").and_then(Value::as_u64).unwrap_or(2),
        "httpStreamWorkerMin": value.get("http_stream_worker_min").and_then(Value::as_u64).unwrap_or(2),
    })
}

fn config_write_response(config: &Value, file_path: PathBuf) -> Result<Value, ConfigWriteFailure> {
    let version = config_version(config)
        .map_err(|message| config_write_failure("configValidationError", message))?;
    Ok(serde_json::json!({
        "status": "ok",
        "version": version,
        "filePath": file_path,
        "overriddenMetadata": Value::Null,
    }))
}

fn config_write_failure(
    config_write_error_code: &'static str,
    message: impl Into<String>,
) -> ConfigWriteFailure {
    ConfigWriteFailure {
        message: message.into(),
        config_write_error_code,
    }
}

fn required_bool(snapshot: &Value, key: &str) -> Result<bool, String> {
    snapshot
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("missing boolean app setting: {key}"))
}

fn required_u64(snapshot: &Value, key: &str) -> Result<u64, String> {
    snapshot
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing integer app setting: {key}"))
}

fn required_string(snapshot: &Value, key: &str) -> Result<String, String> {
    snapshot
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing string app setting: {key}"))
}

fn optional_string(snapshot: &Value, key: &str) -> Option<String> {
    snapshot
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn required_bool_path(config: &Value, path: &[&str]) -> Result<bool, ConfigWriteFailure> {
    get_path_value(config, path)
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            config_write_failure(
                "configValidationError",
                format!("config path {} must be a boolean", path.join(".")),
            )
        })
}

fn required_u64_path(config: &Value, path: &[&str]) -> Result<u64, ConfigWriteFailure> {
    get_path_value(config, path)
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            config_write_failure(
                "configValidationError",
                format!("config path {} must be an integer", path.join(".")),
            )
        })
}

fn required_string_path(config: &Value, path: &[&str]) -> Result<String, ConfigWriteFailure> {
    get_path_value(config, path)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            config_write_failure(
                "configValidationError",
                format!("config path {} must be a string", path.join(".")),
            )
        })
}

fn optional_string_path(config: &Value, path: &[&str]) -> Option<String> {
    get_path_value(config, path)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn required_object_path(config: &Value, path: &[&str]) -> Result<Value, ConfigWriteFailure> {
    get_path_value(config, path)
        .and_then(Value::as_object)
        .map(|value| Value::Object(value.clone()))
        .ok_or_else(|| {
            config_write_failure(
                "configValidationError",
                format!("config path {} must be an object", path.join(".")),
            )
        })
}

fn get_path_value<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for segment in path {
        current = current.get(*segment)?;
    }
    Some(current)
}

fn skills_list(params: Option<&Value>) -> Result<Value, String> {
    let cwds = parse_skill_cwds(params)?;
    let extra_user_roots = parse_per_cwd_extra_user_roots(params)?;
    let enabled_overrides = load_skill_enabled_overrides();
    let default_cwd =
        std::env::current_dir().map_err(|err| format!("read current dir failed: {err}"))?;
    let cwds = if cwds.is_empty() {
        vec![default_cwd]
    } else {
        cwds
    };

    let mut data = Vec::new();
    for cwd in cwds {
        let mut skills = Vec::new();
        let mut errors = Vec::new();
        let mut seen_paths = BTreeSet::new();

        if let Some(user_root) = default_user_skill_root() {
            discover_skills_under(
                &user_root,
                "user",
                &enabled_overrides,
                &mut seen_paths,
                &mut skills,
                &mut errors,
            );
        }
        if let Some(extra_roots) = extra_user_roots.get(&cwd) {
            for root in extra_roots {
                discover_skills_under(
                    root,
                    "user",
                    &enabled_overrides,
                    &mut seen_paths,
                    &mut skills,
                    &mut errors,
                );
            }
        }
        discover_skills_under(
            &cwd.join(".codex").join("skills"),
            "repo",
            &enabled_overrides,
            &mut seen_paths,
            &mut skills,
            &mut errors,
        );

        data.push(serde_json::json!({
            "cwd": cwd,
            "skills": skills,
            "errors": errors
        }));
    }

    Ok(serde_json::json!({ "data": data }))
}

fn skills_config_write(params: Option<&Value>) -> Result<Value, String> {
    let value = params.ok_or_else(|| "skills/config/write params are required".to_string())?;
    let params = serde_json::from_value::<SkillsConfigWriteParams>(value.clone())
        .map_err(|err| format!("invalid skills/config/write params: {err}"))?;
    let skill_path = normalize_skill_config_path(&params.path)?;
    let mut overrides = load_skill_enabled_overrides();
    overrides.insert(skill_path, params.enabled);
    save_skill_enabled_overrides(&overrides)?;
    Ok(serde_json::json!({
        "effectiveEnabled": params.enabled
    }))
}

fn parse_skill_cwds(params: Option<&Value>) -> Result<Vec<PathBuf>, String> {
    let Some(params) = params else {
        return Ok(Vec::new());
    };
    let Some(cwds) = params.get("cwds") else {
        return Ok(Vec::new());
    };
    let Some(items) = cwds.as_array() else {
        return Err("skills/list params.cwds must be an array".to_string());
    };

    let mut out = Vec::new();
    for item in items {
        let Some(cwd) = item.as_str() else {
            return Err("skills/list params.cwds entries must be strings".to_string());
        };
        let cwd = cwd.trim();
        if cwd.is_empty() {
            continue;
        }
        out.push(PathBuf::from(cwd));
    }
    Ok(out)
}

fn parse_per_cwd_extra_user_roots(
    params: Option<&Value>,
) -> Result<std::collections::BTreeMap<PathBuf, Vec<PathBuf>>, String> {
    let Some(params) = params else {
        return Ok(Default::default());
    };
    let Some(entries) = params.get("perCwdExtraUserRoots") else {
        return Ok(Default::default());
    };
    let Some(entries) = entries.as_array() else {
        return Err("skills/list params.perCwdExtraUserRoots must be an array".to_string());
    };
    let mut out = std::collections::BTreeMap::<PathBuf, Vec<PathBuf>>::new();
    for entry in entries {
        let Some(entry_obj) = entry.as_object() else {
            return Err(
                "skills/list params.perCwdExtraUserRoots entries must be objects".to_string(),
            );
        };
        let cwd = entry_obj
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "skills/list params.perCwdExtraUserRoots.cwd is required".to_string())?;
        let extra_roots = entry_obj
            .get("extraUserRoots")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                "skills/list params.perCwdExtraUserRoots.extraUserRoots must be an array"
                    .to_string()
            })?;
        let roots = extra_roots
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        out.insert(PathBuf::from(cwd), roots);
    }
    Ok(out)
}

fn default_user_skill_root() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".codex"))
        })
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .map(|home| home.join(".codex"))
        })
        .map(|root| root.join("skills"))
}

fn discover_skills_under(
    root: &Path,
    scope: &str,
    enabled_overrides: &BTreeMap<String, bool>,
    seen_paths: &mut BTreeSet<String>,
    skills: &mut Vec<Value>,
    errors: &mut Vec<String>,
) {
    if !root.exists() {
        return;
    }
    let root = match root.canonicalize() {
        Ok(value) => value,
        Err(err) => {
            errors.push(format!("scan {} failed: {err}", root.display()));
            return;
        }
    };

    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let skill_file = dir.join(DEFAULT_SKILL_FILE_NAME);
        if skill_file.is_file() {
            let canonical_skill = skill_file
                .canonicalize()
                .unwrap_or_else(|_| skill_file.clone())
                .to_string_lossy()
                .to_string();
            if !seen_paths.insert(canonical_skill.clone()) {
                continue;
            }
            skills.push(build_skill_metadata(&skill_file, scope, enabled_overrides));
            continue;
        }

        let read_dir = match fs::read_dir(&dir) {
            Ok(value) => value,
            Err(err) => {
                errors.push(format!("scan {} failed: {err}", dir.display()));
                continue;
            }
        };
        for child in read_dir.flatten() {
            let path = child.path();
            if path.is_dir() {
                stack.push(path);
            }
        }
    }

    skills.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .cmp(&right.get("name").and_then(Value::as_str))
    });
}

fn build_skill_metadata(
    skill_file: &Path,
    scope: &str,
    enabled_overrides: &BTreeMap<String, bool>,
) -> Value {
    let skill_name = skill_file
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| "unknown-skill".to_string());
    let description = fs::read_to_string(skill_file)
        .ok()
        .and_then(|content| {
            content
                .lines()
                .map(str::trim)
                .find(|line| {
                    !line.is_empty()
                        && !line.starts_with('#')
                        && !line.starts_with('-')
                        && !line.starts_with("```")
                })
                .map(str::to_string)
        })
        .unwrap_or_default();
    let enabled = enabled_overrides
        .get(&canonical_skill_key(skill_file))
        .copied()
        .unwrap_or(true);

    serde_json::json!({
        "name": skill_name,
        "description": description,
        "shortDescription": Value::Null,
        "interface": Value::Null,
        "dependencies": Value::Null,
        "path": skill_file,
        "scope": scope,
        "enabled": enabled
    })
}

fn load_skill_enabled_overrides() -> BTreeMap<String, bool> {
    let Ok(path) = skill_config_file_path() else {
        return BTreeMap::new();
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    serde_json::from_str::<BTreeMap<String, bool>>(&raw).unwrap_or_default()
}

fn save_skill_enabled_overrides(overrides: &BTreeMap<String, bool>) -> Result<(), String> {
    let path = skill_config_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("create skill config dir {} failed: {err}", parent.display()))?;
    }
    let raw = serde_json::to_vec_pretty(overrides)
        .map_err(|err| format!("serialize skill config failed: {err}"))?;
    fs::write(&path, raw)
        .map_err(|err| format!("write skill config {} failed: {err}", path.display()))
}

fn skill_config_file_path() -> Result<PathBuf, String> {
    let compat_config_path = compat_config_file_path()?;
    let parent = compat_config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(parent.join(DEFAULT_SKILL_CONFIG_FILE_NAME))
}

fn canonical_skill_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn normalize_skill_config_path(path: &Path) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!("skill path is not a file: {}", path.display()));
    }
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !file_name.eq_ignore_ascii_case(DEFAULT_SKILL_FILE_NAME) {
        return Err(format!(
            "skill path must point to {}: {}",
            DEFAULT_SKILL_FILE_NAME,
            path.display()
        ));
    }
    Ok(canonical_skill_key(path))
}
