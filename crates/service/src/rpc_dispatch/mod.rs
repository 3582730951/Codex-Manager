use codexmanager_core::rpc::types::{InitializeResult, JsonRpcRequest, JsonRpcResponse};
use codexmanager_core::storage::{now_ts, Event};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{OnceLock, RwLock};

use crate::storage_helpers;

mod account;
mod apikey;
mod app_settings;
mod codex_compat;
mod gateway;
mod requestlog;
mod service_config;
mod startup;
mod usage;

const RPC_CONNECTION_SESSION_TTL_SECS: i64 = 30 * 60;
const RPC_CONNECTION_SESSION_LIMIT: usize = 256;

#[derive(Debug, Clone, Default)]
pub(crate) struct RpcRequestContext {
    pub(crate) connection_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RpcConnectionPhase {
    PendingInitializedAck,
    Ready,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct InitializeClientInfo {
    name: Option<String>,
    title: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct InitializeCapabilities {
    opt_out_notification_methods: Vec<String>,
    experimental_api: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct InitializeParams {
    client_info: Option<InitializeClientInfo>,
    capabilities: InitializeCapabilities,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct RpcConnectionSession {
    phase: RpcConnectionPhase,
    touched_at: i64,
    experimental_api: bool,
    client_name: Option<String>,
    client_title: Option<String>,
    client_version: Option<String>,
    opt_out_notification_methods: BTreeSet<String>,
}

fn rpc_connection_sessions() -> &'static RwLock<BTreeMap<String, RpcConnectionSession>> {
    static SESSIONS: OnceLock<RwLock<BTreeMap<String, RpcConnectionSession>>> = OnceLock::new();
    SESSIONS.get_or_init(|| RwLock::new(BTreeMap::new()))
}

#[cfg(test)]
fn clear_connection_sessions() {
    crate::lock_utils::write_recover(rpc_connection_sessions(), "rpc_connection_sessions").clear();
}

pub(super) fn response(req: &JsonRpcRequest, result: Value) -> JsonRpcResponse {
    JsonRpcResponse { id: req.id, result }
}

pub(super) fn as_json<T: Serialize>(value: T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

pub(super) fn str_param<'a>(req: &'a JsonRpcRequest, key: &str) -> Option<&'a str> {
    req.params
        .as_ref()
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_str())
}

pub(super) fn string_param(req: &JsonRpcRequest, key: &str) -> Option<String> {
    str_param(req, key).map(|v| v.to_string())
}

pub(super) fn i64_param(req: &JsonRpcRequest, key: &str) -> Option<i64> {
    req.params
        .as_ref()
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_i64())
}

pub(super) fn bool_param(req: &JsonRpcRequest, key: &str) -> Option<bool> {
    req.params
        .as_ref()
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_bool())
}

pub(super) fn ok_result() -> Value {
    serde_json::json!({ "ok": true })
}

pub(super) fn ok_or_error(result: Result<(), String>) -> Value {
    match result {
        Ok(_) => ok_result(),
        Err(err) => crate::error_codes::rpc_action_error_payload(err),
    }
}

pub(super) fn value_or_error<T: Serialize>(result: Result<T, String>) -> Value {
    match result {
        Ok(value) => as_json(value),
        Err(err) => crate::error_codes::rpc_error_payload(err),
    }
}

fn cleanup_expired_sessions(sessions: &mut BTreeMap<String, RpcConnectionSession>, now: i64) {
    sessions.retain(|_, session| {
        now.saturating_sub(session.touched_at) <= RPC_CONNECTION_SESSION_TTL_SECS
    });
    while sessions.len() > RPC_CONNECTION_SESSION_LIMIT {
        let Some(oldest_key) = sessions
            .iter()
            .min_by_key(|(_, session)| session.touched_at)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        sessions.remove(&oldest_key);
    }
}

fn parse_initialize_params(params: Option<&Value>) -> Result<InitializeParams, String> {
    match params {
        Some(value) => serde_json::from_value::<InitializeParams>(value.clone())
            .map_err(|err| format!("invalid initialize params: {err}")),
        None => Ok(InitializeParams::default()),
    }
}

fn normalize_opt_out_notification_methods(methods: Vec<String>) -> BTreeSet<String> {
    methods
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn begin_connection_session(connection_id: &str, params: Option<&Value>) -> Result<(), String> {
    let initialize = parse_initialize_params(params)?;
    let now = now_ts();
    let mut sessions =
        crate::lock_utils::write_recover(rpc_connection_sessions(), "rpc_connection_sessions");
    cleanup_expired_sessions(&mut sessions, now);
    if sessions.contains_key(connection_id) {
        return Err("Already initialized".to_string());
    }
    sessions.insert(
        connection_id.to_string(),
        RpcConnectionSession {
            phase: RpcConnectionPhase::PendingInitializedAck,
            touched_at: now,
            experimental_api: initialize.capabilities.experimental_api,
            client_name: initialize
                .client_info
                .as_ref()
                .and_then(|value| value.name.clone()),
            client_title: initialize
                .client_info
                .as_ref()
                .and_then(|value| value.title.clone()),
            client_version: initialize
                .client_info
                .as_ref()
                .and_then(|value| value.version.clone()),
            opt_out_notification_methods: normalize_opt_out_notification_methods(
                initialize.capabilities.opt_out_notification_methods,
            ),
        },
    );
    Ok(())
}

fn mark_connection_initialized(connection_id: &str) -> Result<(), String> {
    let now = now_ts();
    let mut sessions =
        crate::lock_utils::write_recover(rpc_connection_sessions(), "rpc_connection_sessions");
    cleanup_expired_sessions(&mut sessions, now);
    let Some(session) = sessions.get_mut(connection_id) else {
        return Err("Not initialized".to_string());
    };
    session.phase = RpcConnectionPhase::Ready;
    session.touched_at = now;
    Ok(())
}

fn ensure_connection_ready(connection_id: &str) -> Result<(), String> {
    let now = now_ts();
    let mut sessions =
        crate::lock_utils::write_recover(rpc_connection_sessions(), "rpc_connection_sessions");
    cleanup_expired_sessions(&mut sessions, now);
    let Some(session) = sessions.get_mut(connection_id) else {
        return Err("Not initialized".to_string());
    };
    if session.phase != RpcConnectionPhase::Ready {
        return Err("Not initialized".to_string());
    }
    session.touched_at = now;
    Ok(())
}

fn enforce_connection_handshake(
    req: &JsonRpcRequest,
    ctx: &RpcRequestContext,
) -> Option<JsonRpcResponse> {
    let Some(connection_id) = ctx.connection_id.as_deref() else {
        return None;
    };

    let gate = match req.method.as_str() {
        "initialize" => begin_connection_session(connection_id, req.params.as_ref()),
        "initialized" => mark_connection_initialized(connection_id),
        _ => ensure_connection_ready(connection_id),
    };

    gate.err()
        .map(crate::error_codes::rpc_error_payload)
        .map(|result| response(req, result))
}

pub(crate) fn handle_request_with_context(
    req: JsonRpcRequest,
    ctx: &RpcRequestContext,
) -> JsonRpcResponse {
    if let Some(resp) = enforce_connection_handshake(&req, ctx) {
        return resp;
    }

    if req.method == "initialize" {
        let _ = storage_helpers::initialize_storage();
        if let Some(storage) = storage_helpers::open_storage() {
            let _ = storage.insert_event(&Event {
                account_id: None,
                event_type: "initialize".to_string(),
                message: "service initialized".to_string(),
                created_at: now_ts(),
            });
        }
        let result = InitializeResult {
            server_name: "codexmanager-service".to_string(),
            version: codexmanager_core::core_version().to_string(),
            user_agent: crate::gateway::current_codex_user_agent(),
        };
        return response(&req, as_json(result));
    }
    if let Some(resp) = codex_compat::try_handle(&req) {
        return resp;
    }

    if let Some(resp) = account::try_handle(&req) {
        return resp;
    }
    if let Some(resp) = apikey::try_handle(&req) {
        return resp;
    }
    if let Some(resp) = app_settings::try_handle(&req) {
        return resp;
    }
    if let Some(resp) = usage::try_handle(&req) {
        return resp;
    }
    if let Some(resp) = service_config::try_handle(&req) {
        return resp;
    }
    if let Some(resp) = startup::try_handle(&req) {
        return resp;
    }
    if let Some(resp) = gateway::try_handle(&req) {
        return resp;
    }
    if let Some(resp) = requestlog::try_handle(&req) {
        return resp;
    }

    response(
        &req,
        crate::error_codes::rpc_error_payload("unknown_method".to_string()),
    )
}

#[allow(dead_code)]
pub(crate) fn handle_request(req: JsonRpcRequest) -> JsonRpcResponse {
    handle_request_with_context(req, &RpcRequestContext::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: u64, method: &str) -> JsonRpcRequest {
        JsonRpcRequest {
            id,
            method: method.to_string(),
            params: None,
        }
    }

    #[test]
    fn connection_session_requires_initialize_then_initialized() {
        clear_connection_sessions();
        let ctx = RpcRequestContext {
            connection_id: Some("rpc-session-test".to_string()),
        };

        let resp = enforce_connection_handshake(&req(1, "skills/list"), &ctx).expect("reject");
        assert_eq!(resp.result["error"], "Not initialized");

        assert!(enforce_connection_handshake(&req(2, "initialize"), &ctx).is_none());

        let resp = enforce_connection_handshake(&req(3, "skills/list"), &ctx).expect("pending");
        assert_eq!(resp.result["error"], "Not initialized");

        assert!(enforce_connection_handshake(&req(4, "initialized"), &ctx).is_none());
        assert!(enforce_connection_handshake(&req(5, "skills/list"), &ctx).is_none());

        let resp = enforce_connection_handshake(&req(6, "initialize"), &ctx).expect("repeat");
        assert_eq!(resp.result["error"], "Already initialized");

        clear_connection_sessions();
    }

    #[test]
    fn missing_connection_id_skips_handshake_gate() {
        clear_connection_sessions();
        let ctx = RpcRequestContext::default();
        assert!(enforce_connection_handshake(&req(7, "skills/list"), &ctx).is_none());
    }

    #[test]
    fn initialize_session_persists_client_info_and_notification_preferences() {
        clear_connection_sessions();
        let ctx = RpcRequestContext {
            connection_id: Some("rpc-session-metadata".to_string()),
        };
        let req = JsonRpcRequest {
            id: 8,
            method: "initialize".to_string(),
            params: Some(serde_json::json!({
                "clientInfo": {
                    "name": "codex_vscode",
                    "title": "Codex VS Code Extension",
                    "version": "0.1.0"
                },
                "capabilities": {
                    "experimentalApi": true,
                    "optOutNotificationMethods": [
                        "skills/changed",
                        "account/updated"
                    ]
                }
            })),
        };

        assert!(enforce_connection_handshake(&req, &ctx).is_none());

        let sessions =
            crate::lock_utils::read_recover(rpc_connection_sessions(), "rpc_connection_sessions");
        let session = sessions
            .get("rpc-session-metadata")
            .expect("connection session");
        assert_eq!(session.client_name.as_deref(), Some("codex_vscode"));
        assert_eq!(
            session.client_title.as_deref(),
            Some("Codex VS Code Extension")
        );
        assert_eq!(session.client_version.as_deref(), Some("0.1.0"));
        assert!(session.experimental_api);
        assert!(session
            .opt_out_notification_methods
            .contains("skills/changed"));
        assert!(session
            .opt_out_notification_methods
            .contains("account/updated"));

        drop(sessions);
        clear_connection_sessions();
    }
}
