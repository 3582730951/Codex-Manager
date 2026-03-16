use super::*;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct ServiceStartRequest {
    addr: Option<String>,
}

pub(super) fn should_spawn_service() -> bool {
    read_env_trim("CODEXMANAGER_WEB_NO_SPAWN_SERVICE").is_none()
}

pub(super) async fn tcp_probe(addr: &str) -> bool {
    let addr = addr.trim();
    if addr.is_empty() {
        return false;
    }
    let addr = addr.strip_prefix("http://").unwrap_or(addr);
    let addr = addr.strip_prefix("https://").unwrap_or(addr);
    let addr = addr.split('/').next().unwrap_or(addr);
    tokio::time::timeout(
        Duration::from_millis(250),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    .is_ok()
}

async fn rpc_probe_ready(addr: &str, rpc_token: &str) -> bool {
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let response = client
        .post(service_rpc_url(addr))
        .header("content-type", "application/json")
        .header("x-codexmanager-rpc-token", rpc_token)
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
        .send()
        .await;
    let Ok(response) = response else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(payload) = response.json::<serde_json::Value>().await else {
        return false;
    };
    payload
        .get("result")
        .and_then(|result| result.get("server_name").or_else(|| result.get("serverName")))
        .and_then(|value| value.as_str())
        .map(|value| value == "codexmanager-service")
        .unwrap_or(false)
}

async fn wait_for_rpc_ready(addr: &str, rpc_token: &str, attempts: usize) -> bool {
    for _ in 0..attempts {
        if rpc_probe_ready(addr, rpc_token).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn service_bin_path(dir: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        return dir.join("codexmanager-service.exe");
    }
    #[cfg(not(target_os = "windows"))]
    {
        return dir.join("codexmanager-service");
    }
}

fn spawn_service_detached(dir: &Path, service_addr: &str) -> std::io::Result<()> {
    let bin = service_bin_path(dir);
    let mut cmd = Command::new(bin);
    let bind_addr = codexmanager_service::listener_bind_addr(service_addr);
    cmd.env("CODEXMANAGER_SERVICE_ADDR", bind_addr);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let _child = cmd.spawn()?;
    Ok(())
}

pub(super) async fn ensure_service_running(
    service_addr: &str,
    rpc_token: &str,
    dir: &Path,
    spawned_service: &Arc<Mutex<bool>>,
) -> Option<String> {
    if rpc_probe_ready(service_addr, rpc_token).await {
        return None;
    }
    if tcp_probe(service_addr).await {
        if wait_for_rpc_ready(service_addr, rpc_token, 50).await {
            return None;
        }
        return Some(format!(
            "service reachable at {service_addr} but RPC initialize is not ready"
        ));
    }
    if !should_spawn_service() {
        return Some(format!(
            "service not reachable at {service_addr} (spawn disabled)"
        ));
    }

    let bin = service_bin_path(dir);
    if !bin.is_file() {
        return Some(format!(
            "service not reachable at {service_addr} (missing {})",
            bin.display()
        ));
    }

    if let Err(err) = spawn_service_detached(dir, service_addr) {
        return Some(format!("failed to spawn service: {err}"));
    }
    *spawned_service.lock().await = true;

    if wait_for_rpc_ready(service_addr, rpc_token, 50).await {
        return None;
    }
    Some(format!(
        "service still not RPC-ready at {service_addr} after spawn"
    ))
}

async fn current_service_addr(state: &Arc<AppState>) -> String {
    state.service_addr.read().await.clone()
}

async fn update_service_target(state: &Arc<AppState>, addr: &str) {
    *state.service_addr.write().await = addr.to_string();
    *state.service_rpc_url.write().await = service_rpc_url(addr);
}

fn json_error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        axum::Json(json!({
            "ok": false,
            "error": message.into(),
        })),
    )
        .into_response()
}

fn json_ok(payload: serde_json::Value) -> Response {
    axum::Json(payload).into_response()
}

pub(super) async fn rpc_proxy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_json_content_type(&headers) {
        return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "{}").into_response();
    }
    let service_rpc_url = state.service_rpc_url.read().await.clone();
    let resp = state
        .client
        .post(&service_rpc_url)
        .header("content-type", "application/json")
        .header("x-codexmanager-rpc-token", &state.rpc_token)
        .body(body)
        .send()
        .await;
    let resp = match resp {
        Ok(v) => v,
        Err(err) => {
            let msg = format!("upstream error: {err}");
            return (StatusCode::BAD_GATEWAY, msg).into_response();
        }
    };

    let status = resp.status();
    let bytes = match resp.bytes().await {
        Ok(v) => v,
        Err(err) => {
            let msg = format!("upstream read error: {err}");
            return (StatusCode::BAD_GATEWAY, msg).into_response();
        }
    };
    let mut out = Response::new(axum::body::Body::from(bytes));
    *out.status_mut() = status;
    out.headers_mut().insert(
        "content-type",
        axum::http::HeaderValue::from_static("application/json"),
    );
    out
}

pub(super) async fn start_service(
    State(state): State<Arc<AppState>>,
    axum::Json(payload): axum::Json<ServiceStartRequest>,
) -> Response {
    let requested_addr = match payload.addr.as_deref() {
        Some(raw) => match normalize_addr(raw) {
            Some(value) => value,
            None => return json_error(StatusCode::BAD_REQUEST, "invalid service address"),
        },
        None => current_service_addr(&state).await,
    };

    if rpc_probe_ready(&requested_addr, &state.rpc_token).await {
        update_service_target(&state, &requested_addr).await;
        return json_ok(json!({
            "ok": true,
            "addr": requested_addr,
            "started": false,
        }));
    }
    if tcp_probe(&requested_addr).await {
        if wait_for_rpc_ready(&requested_addr, &state.rpc_token, 50).await {
            update_service_target(&state, &requested_addr).await;
            return json_ok(json!({
                "ok": true,
                "addr": requested_addr,
                "started": false,
            }));
        }
        return json_error(
            StatusCode::BAD_GATEWAY,
            format!("service reachable at {requested_addr} but RPC initialize is not ready"),
        );
    }

    if !should_spawn_service() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("service not reachable at {requested_addr} (spawn disabled)"),
        );
    }

    let dir = exe_dir();
    let bin = service_bin_path(&dir);
    if !bin.is_file() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "service not reachable at {requested_addr} (missing {})",
                bin.display()
            ),
        );
    }

    if let Err(err) = spawn_service_detached(&dir, &requested_addr) {
        return json_error(
            StatusCode::BAD_GATEWAY,
            format!("failed to spawn service: {err}"),
        );
    }
    *state.spawned_service.lock().await = true;

    if wait_for_rpc_ready(&requested_addr, &state.rpc_token, 50).await {
        update_service_target(&state, &requested_addr).await;
        return json_ok(json!({
            "ok": true,
            "addr": requested_addr,
            "started": true,
        }));
    }

    json_error(
        StatusCode::BAD_GATEWAY,
        format!("service still not RPC-ready at {requested_addr} after spawn"),
    )
}

pub(super) async fn stop_service(State(state): State<Arc<AppState>>) -> Response {
    let addr = current_service_addr(&state).await;
    let _ = tokio::task::spawn_blocking(move || {
        codexmanager_service::request_shutdown(&addr);
    })
    .await;
    *state.spawned_service.lock().await = false;
    json_ok(json!({ "ok": true }))
}

pub(super) async fn quit(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if *state.spawned_service.lock().await {
        let addr = current_service_addr(&state).await;
        let _ = tokio::task::spawn_blocking(move || {
            codexmanager_service::request_shutdown(&addr);
        })
        .await;
    }
    let _ = state.shutdown_tx.send(true);
    Html("<html><body>OK</body></html>")
}
