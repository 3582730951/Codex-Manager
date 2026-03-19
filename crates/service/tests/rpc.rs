use codexmanager_core::auth::{DEFAULT_CLIENT_ID, DEFAULT_ISSUER};
use codexmanager_core::rpc::types::JsonRpcRequest;
use codexmanager_core::storage::{
    now_ts, Account, RequestLog, RequestTokenStat, Storage, UsageSnapshotRecord,
};
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Duration;
use tiny_http::{Header, Response, Server, StatusCode};

struct EnvGuard {
    key: &'static str,
    original: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, original }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.original {
            std::env::set_var(self.key, value);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

static RPC_TEST_ENV_LOCK: Mutex<()> = Mutex::new(());
static RPC_TEST_DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

fn lock_rpc_test_env() -> MutexGuard<'static, ()> {
    // 中文注释：RPC 集成测试依赖进程级环境变量，串行化可避免不同用例互相污染数据库路径。
    RPC_TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn new_test_dir(prefix: &str) -> PathBuf {
    // 中文注释：用进程号 + 自增序号构造临时目录，避免 Windows 复用旧目录导致脏数据串用。
    let seq = RPC_TEST_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut dir = std::env::temp_dir();
    dir.push(format!("{prefix}-{}-{seq}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    dir
}

struct RpcTestContext {
    _env_lock: MutexGuard<'static, ()>,
    _db_path_guard: EnvGuard,
    dir: PathBuf,
}

impl RpcTestContext {
    fn new(prefix: &str) -> Self {
        let env_lock = lock_rpc_test_env();
        let dir = new_test_dir(prefix);
        let db_path = dir.join("codexmanager.db");
        let db_path_guard =
            EnvGuard::set("CODEXMANAGER_DB_PATH", db_path.to_string_lossy().as_ref());
        Self {
            _env_lock: env_lock,
            _db_path_guard: db_path_guard,
            dir,
        }
    }

    fn db_path(&self) -> PathBuf {
        self.dir.join("codexmanager.db")
    }

    fn seed_accounts(&self, count: usize) {
        let storage = Storage::open(self.db_path()).expect("open db");
        storage.init().expect("init schema");
        let now = now_ts();
        for idx in 0..count {
            let sort = idx as i64;
            storage
                .insert_account(&Account {
                    id: format!("acc-{idx}"),
                    label: format!("Account {idx}"),
                    issuer: "https://auth.openai.com".to_string(),
                    chatgpt_account_id: Some(format!("chatgpt-{idx}")),
                    workspace_id: Some(format!("workspace-{idx}")),
                    group_name: Some(format!("group-{}", idx % 2)),
                    sort,
                    status: "active".to_string(),
                    created_at: now + sort,
                    updated_at: now + sort,
                })
                .expect("insert account");
        }
    }
}

impl Drop for RpcTestContext {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn post_rpc_raw(addr: &str, body: &str, headers: &[(&str, &str)]) -> (u16, String) {
    let mut stream = {
        let mut last_err = None;
        let mut connected = None;
        for _ in 0..20 {
            match TcpStream::connect(addr) {
                Ok(stream) => {
                    connected = Some(stream);
                    break;
                }
                Err(err) => {
                    last_err = Some(err);
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
        connected.unwrap_or_else(|| {
            panic!(
                "connect server: {}",
                last_err
                    .map(|err| err.to_string())
                    .unwrap_or_else(|| "unknown error".to_string())
            )
        })
    };
    let mut request = format!("POST /rpc HTTP/1.1\r\nHost: {addr}\r\n");
    for (name, value) in headers {
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n{}", body.len(), body));
    stream.write_all(request.as_bytes()).expect("write");
    stream.shutdown(std::net::Shutdown::Write).ok();

    let mut buf = String::new();
    stream.read_to_string(&mut buf).expect("read");
    let status = buf
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .expect("status");
    let body = buf.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

fn post_rpc(addr: &str, body: &str) -> serde_json::Value {
    let token = codexmanager_service::rpc_auth_token().to_string();
    let (status, body) = post_rpc_raw(
        addr,
        body,
        &[
            ("Content-Type", "application/json"),
            ("X-CodexManager-Rpc-Token", token.as_str()),
        ],
    );
    assert_eq!(status, 200, "unexpected status {status}: {body}");
    serde_json::from_str(&body).expect("parse response")
}

fn encode_base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let chunk = ((bytes[index] as u32) << 16)
            | ((bytes[index + 1] as u32) << 8)
            | (bytes[index + 2] as u32);
        out.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
        out.push(TABLE[((chunk >> 6) & 0x3f) as usize] as char);
        out.push(TABLE[(chunk & 0x3f) as usize] as char);
        index += 3;
    }
    match bytes.len().saturating_sub(index) {
        1 => {
            let chunk = (bytes[index] as u32) << 16;
            out.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
            out.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
        }
        2 => {
            let chunk = ((bytes[index] as u32) << 16) | ((bytes[index + 1] as u32) << 8);
            out.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
            out.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
            out.push(TABLE[((chunk >> 6) & 0x3f) as usize] as char);
        }
        _ => {}
    }
    out
}

fn build_access_token(
    subject: &str,
    email: &str,
    chatgpt_account_id: &str,
    plan_type: &str,
) -> String {
    let header = encode_base64url(br#"{"alg":"none","typ":"JWT"}"#);
    let payload = serde_json::json!({
        "sub": subject,
        "email": email,
        "workspace_id": chatgpt_account_id,
        "https://api.openai.com/auth": {
            "chatgpt_account_id": chatgpt_account_id,
            "chatgpt_plan_type": plan_type
        }
    });
    let payload = encode_base64url(
        serde_json::to_string(&payload)
            .expect("serialize jwt payload")
            .as_bytes(),
    );
    format!("{header}.{payload}.sig")
}

fn start_mock_oauth_token_server(
    status: u16,
    response_body: String,
) -> (
    String,
    std::sync::mpsc::Receiver<String>,
    thread::JoinHandle<()>,
) {
    let server = Server::http("127.0.0.1:0").expect("start mock oauth server");
    let addr = format!("http://{}", server.server_addr());
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = thread::spawn(move || {
        let mut request = server.recv().expect("receive oauth request");
        let mut body = String::new();
        request
            .as_reader()
            .read_to_string(&mut body)
            .expect("read oauth request body");
        tx.send(body).expect("send oauth request body");
        let response = Response::from_string(response_body)
            .with_status_code(StatusCode(status))
            .with_header(
                Header::from_bytes("Content-Type", "application/json")
                    .expect("content-type header"),
            );
        request.respond(response).expect("respond oauth request");
    });
    (addr, rx, handle)
}

#[test]
fn rpc_initialize_roundtrip() {
    let _ctx = RpcTestContext::new("rpc-initialize");
    let server = codexmanager_service::start_one_shot_server().expect("start server");

    let req = JsonRpcRequest {
        id: 1,
        method: "initialize".to_string(),
        params: None,
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let v = post_rpc(&server.addr, &json);
    let result = v.get("result").expect("result");
    assert_eq!(result.get("server_name").unwrap(), "codexmanager-service");
    assert!(result
        .get("user_agent")
        .and_then(|value| value.as_str())
        .is_some_and(|value| value.contains("/0.101.0") && value.contains("CodexManagerGateway")));
}

#[test]
fn rpc_codex_compat_list_methods_return_stable_shapes() {
    let _ctx = RpcTestContext::new("rpc-codex-compat");
    let initialized_server = codexmanager_service::start_one_shot_server().expect("start server");
    let initialized = post_rpc(
        &initialized_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 101,
            method: "initialized".to_string(),
            params: None,
        })
        .expect("serialize initialized"),
    );
    assert_eq!(
        initialized
            .get("result")
            .and_then(|value| value.get("ok"))
            .and_then(|value| value.as_bool()),
        Some(true)
    );

    let experimental_server = codexmanager_service::start_one_shot_server().expect("start server");
    let experimental = post_rpc(
        &experimental_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 102,
            method: "experimentalFeature/list".to_string(),
            params: Some(serde_json::json!({ "limit": 2 })),
        })
        .expect("serialize experimental feature list"),
    );
    let experimental_items = experimental["result"]["data"]
        .as_array()
        .expect("experimental features array");
    assert_eq!(experimental_items.len(), 2);
    assert_eq!(experimental["result"]["nextCursor"], Value::Null);

    let collaboration_server = codexmanager_service::start_one_shot_server().expect("start server");
    let collaboration = post_rpc(
        &collaboration_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 103,
            method: "collaborationMode/list".to_string(),
            params: None,
        })
        .expect("serialize collaboration modes"),
    );
    let collaboration_items = collaboration["result"]["data"]
        .as_array()
        .expect("collaboration modes array");
    assert_eq!(collaboration_items.len(), 2);
    assert_eq!(collaboration_items[0]["name"], "Plan");
    assert_eq!(collaboration_items[1]["name"], "Default");

    let plugin_server = codexmanager_service::start_one_shot_server().expect("start server");
    let plugin_list = post_rpc(
        &plugin_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 104,
            method: "plugin/list".to_string(),
            params: None,
        })
        .expect("serialize plugin list"),
    );
    assert_eq!(plugin_list["result"]["marketplaces"], serde_json::json!([]));
    assert_eq!(plugin_list["result"]["remoteSyncError"], Value::Null);

    let app_server = codexmanager_service::start_one_shot_server().expect("start server");
    let app_list = post_rpc(
        &app_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 105,
            method: "app/list".to_string(),
            params: None,
        })
        .expect("serialize app list"),
    );
    assert_eq!(app_list["result"]["data"], serde_json::json!([]));
    assert_eq!(app_list["result"]["nextCursor"], Value::Null);

    let model_server = codexmanager_service::start_one_shot_server().expect("start server");
    let model_list = post_rpc(
        &model_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 106,
            method: "model/list".to_string(),
            params: Some(serde_json::json!({ "limit": 2 })),
        })
        .expect("serialize model list"),
    );
    assert!(model_list["result"]["data"].is_array());
    assert!(model_list["result"].get("nextCursor").is_some());

    let requirements_server = codexmanager_service::start_one_shot_server().expect("start server");
    let requirements = post_rpc(
        &requirements_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 107,
            method: "configRequirements/read".to_string(),
            params: None,
        })
        .expect("serialize config requirements read"),
    );
    assert_eq!(requirements["result"]["requirements"], Value::Null);

    let mcp_reload_server = codexmanager_service::start_one_shot_server().expect("start server");
    let mcp_reload = post_rpc(
        &mcp_reload_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 108,
            method: "config/mcpServer/reload".to_string(),
            params: None,
        })
        .expect("serialize mcp reload"),
    );
    assert_eq!(mcp_reload["result"], serde_json::json!({}));

    let mcp_status_server = codexmanager_service::start_one_shot_server().expect("start server");
    let mcp_status = post_rpc(
        &mcp_status_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 109,
            method: "mcpServerStatus/list".to_string(),
            params: None,
        })
        .expect("serialize mcp status list"),
    );
    assert_eq!(mcp_status["result"]["data"], serde_json::json!([]));
    assert_eq!(mcp_status["result"]["nextCursor"], Value::Null);

    let external_detect_server =
        codexmanager_service::start_one_shot_server().expect("start server");
    let external_detect = post_rpc(
        &external_detect_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 110,
            method: "externalAgentConfig/detect".to_string(),
            params: Some(serde_json::json!({
                "includeHome": true,
                "cwds": []
            })),
        })
        .expect("serialize external detect"),
    );
    assert_eq!(external_detect["result"]["items"], serde_json::json!([]));

    let external_import_server =
        codexmanager_service::start_one_shot_server().expect("start server");
    let external_import = post_rpc(
        &external_import_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 111,
            method: "externalAgentConfig/import".to_string(),
            params: Some(serde_json::json!({
                "migrationItems": []
            })),
        })
        .expect("serialize external import"),
    );
    assert_eq!(external_import["result"], serde_json::json!({}));

    let skills_remote_list_server =
        codexmanager_service::start_one_shot_server().expect("start server");
    let skills_remote_list = post_rpc(
        &skills_remote_list_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 112,
            method: "skills/remote/list".to_string(),
            params: Some(serde_json::json!({
                "hazelnutScope": "public",
                "productSurface": "codex_cli",
                "enabled": true
            })),
        })
        .expect("serialize skills remote list"),
    );
    assert_eq!(skills_remote_list["result"]["data"], serde_json::json!([]));

    let skills_remote_export_server =
        codexmanager_service::start_one_shot_server().expect("start server");
    let skills_remote_export = post_rpc(
        &skills_remote_export_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 113,
            method: "skills/remote/export".to_string(),
            params: Some(serde_json::json!({
                "hazelnutId": "skill_public_demo"
            })),
        })
        .expect("serialize skills remote export"),
    );
    assert!(skills_remote_export["result"]["error"]
        .as_str()
        .is_some_and(|value| value.contains("remote skill export is not available")));

    let plugin_install_server =
        codexmanager_service::start_one_shot_server().expect("start server");
    let plugin_install = post_rpc(
        &plugin_install_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 114,
            method: "plugin/install".to_string(),
            params: Some(serde_json::json!({
                "marketplacePath": "C:/tmp/marketplace",
                "pluginName": "demo-plugin"
            })),
        })
        .expect("serialize plugin install"),
    );
    assert!(plugin_install["result"]["error"]
        .as_str()
        .is_some_and(|value| value.contains("plugin install is not available")));

    let plugin_uninstall_server =
        codexmanager_service::start_one_shot_server().expect("start server");
    let plugin_uninstall = post_rpc(
        &plugin_uninstall_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 115,
            method: "plugin/uninstall".to_string(),
            params: Some(serde_json::json!({
                "pluginId": "demo-plugin/local"
            })),
        })
        .expect("serialize plugin uninstall"),
    );
    assert!(plugin_uninstall["result"]["error"]
        .as_str()
        .is_some_and(|value| value.contains("plugin uninstall is not available")));

    let mcp_oauth_server = codexmanager_service::start_one_shot_server().expect("start server");
    let mcp_oauth = post_rpc(
        &mcp_oauth_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 116,
            method: "mcpServer/oauth/login".to_string(),
            params: Some(serde_json::json!({
                "name": "demo-mcp",
                "scopes": ["read"],
                "timeoutSecs": 30
            })),
        })
        .expect("serialize mcp oauth login"),
    );
    assert!(mcp_oauth["result"]["error"]
        .as_str()
        .is_some_and(|value| value.contains("mcpServer oauth login is not available")));

    let review_server = codexmanager_service::start_one_shot_server().expect("start server");
    let review_start = post_rpc(
        &review_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 117,
            method: "review/start".to_string(),
            params: Some(serde_json::json!({
                "threadId": "thread_demo",
                "target": { "type": "changes" }
            })),
        })
        .expect("serialize review start"),
    );
    assert!(review_start["result"]["error"]
        .as_str()
        .is_some_and(|value| value.contains("review/start is not available")));
}

#[test]
fn rpc_skills_list_discovers_user_and_repo_skill_roots() {
    let _ctx = RpcTestContext::new("rpc-skills-list");
    let home_dir = new_test_dir("rpc-skills-home");
    let repo_dir = new_test_dir("rpc-skills-repo");
    let _home_guard = EnvGuard::set("HOME", home_dir.to_string_lossy().as_ref());
    let _userprofile_guard = EnvGuard::set("USERPROFILE", home_dir.to_string_lossy().as_ref());

    let user_skill_dir = home_dir.join(".codex").join("skills").join("user-skill");
    let repo_skill_dir = repo_dir.join(".codex").join("skills").join("repo-skill");
    fs::create_dir_all(&user_skill_dir).expect("create user skill dir");
    fs::create_dir_all(&repo_skill_dir).expect("create repo skill dir");
    fs::write(
        user_skill_dir.join("SKILL.md"),
        "# User Skill\n\nUser scoped skill description.",
    )
    .expect("write user skill");
    fs::write(
        repo_skill_dir.join("SKILL.md"),
        "# Repo Skill\n\nRepo scoped skill description.",
    )
    .expect("write repo skill");

    let server = codexmanager_service::start_one_shot_server().expect("start server");
    let response = post_rpc(
        &server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 106,
            method: "skills/list".to_string(),
            params: Some(serde_json::json!({
                "cwds": [repo_dir.to_string_lossy().to_string()]
            })),
        })
        .expect("serialize skills list"),
    );
    let data = response["result"]["data"]
        .as_array()
        .expect("skills data array");
    assert_eq!(data.len(), 1);
    let skills = data[0]["skills"].as_array().expect("skills array");
    let names = skills
        .iter()
        .filter_map(|item| item.get("name").and_then(|value| value.as_str()))
        .collect::<Vec<_>>();
    assert!(names.contains(&"repo-skill"));
    assert!(names.contains(&"user-skill"));

    let repo_skill = skills
        .iter()
        .find(|item| item["name"] == "repo-skill")
        .expect("repo skill");
    assert_eq!(repo_skill["scope"], "repo");
    assert_eq!(repo_skill["description"], "Repo scoped skill description.");

    let user_skill = skills
        .iter()
        .find(|item| item["name"] == "user-skill")
        .expect("user skill");
    assert_eq!(user_skill["scope"], "user");
    assert_eq!(user_skill["description"], "User scoped skill description.");

    let _ = fs::remove_dir_all(&home_dir);
    let _ = fs::remove_dir_all(&repo_dir);
}

#[test]
fn rpc_skills_config_write_persists_enabled_override() {
    let ctx = RpcTestContext::new("rpc-skills-config-write");
    let home_dir = new_test_dir("rpc-skills-config-home");
    let repo_dir = new_test_dir("rpc-skills-config-repo");
    let _home_guard = EnvGuard::set("HOME", home_dir.to_string_lossy().as_ref());
    let _userprofile_guard = EnvGuard::set("USERPROFILE", home_dir.to_string_lossy().as_ref());

    let repo_skill_dir = repo_dir.join(".codex").join("skills").join("repo-skill");
    fs::create_dir_all(&repo_skill_dir).expect("create repo skill dir");
    let repo_skill_file = repo_skill_dir.join("SKILL.md");
    fs::write(
        &repo_skill_file,
        "# Repo Skill\n\nRepo scoped skill description.",
    )
    .expect("write repo skill");

    let write_server = codexmanager_service::start_one_shot_server().expect("start server");
    let write_response = post_rpc(
        &write_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 112,
            method: "skills/config/write".to_string(),
            params: Some(serde_json::json!({
                "path": repo_skill_file.to_string_lossy().to_string(),
                "enabled": false
            })),
        })
        .expect("serialize skills config write"),
    );
    assert_eq!(
        write_response["result"]["effectiveEnabled"],
        Value::Bool(false)
    );

    let skill_config_path = ctx.dir.join("codexmanager.skills-config.json");
    let persisted = fs::read_to_string(&skill_config_path).expect("read skill config");
    let persisted: serde_json::Value =
        serde_json::from_str(&persisted).expect("parse skill config json");
    assert!(persisted
        .as_object()
        .is_some_and(|entries| entries.values().any(|value| value == &Value::Bool(false))));

    let list_server = codexmanager_service::start_one_shot_server().expect("start server");
    let list_response = post_rpc(
        &list_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 113,
            method: "skills/list".to_string(),
            params: Some(serde_json::json!({
                "cwds": [repo_dir.to_string_lossy().to_string()]
            })),
        })
        .expect("serialize skills list"),
    );
    let skills = list_response["result"]["data"][0]["skills"]
        .as_array()
        .expect("skills array");
    let repo_skill = skills
        .iter()
        .find(|item| item["name"] == "repo-skill")
        .expect("repo skill");
    assert_eq!(repo_skill["enabled"], Value::Bool(false));

    let _ = fs::remove_dir_all(&home_dir);
    let _ = fs::remove_dir_all(&repo_dir);
}

#[test]
fn rpc_config_read_returns_supported_config_tree_and_layers() {
    let _ctx = RpcTestContext::new("rpc-config-read");
    let server = codexmanager_service::start_one_shot_server().expect("start server");
    let response = post_rpc(
        &server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 107,
            method: "config/read".to_string(),
            params: Some(serde_json::json!({
                "includeLayers": true
            })),
        })
        .expect("serialize config read"),
    );

    let result = response.get("result").expect("config read result");
    assert!(result["config"]["gateway"]["originator"]
        .as_str()
        .is_some_and(|value| !value.trim().is_empty()));
    assert_eq!(result["config"]["service"]["bind_mode"], "loopback");
    assert_eq!(
        result["config"]["gateway"]["request_compression_enabled"].as_bool(),
        Some(true)
    );
    assert!(result["origins"]
        .get("gateway.originator")
        .and_then(|value| value.get("version"))
        .and_then(|value| value.as_str())
        .is_some_and(|value| value.starts_with("sha256:")));

    let layers = result["layers"].as_array().expect("layers array");
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0]["name"]["type"], "user");
    assert!(layers[0]["name"]["file"]
        .as_str()
        .is_some_and(|value| value.ends_with("codexmanager.compat-config.json")));
}

#[test]
fn rpc_config_value_write_updates_gateway_originator() {
    let _ctx = RpcTestContext::new("rpc-config-value-write");
    let read_server = codexmanager_service::start_one_shot_server().expect("start server");
    let read_response = post_rpc(
        &read_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 108,
            method: "config/read".to_string(),
            params: None,
        })
        .expect("serialize config read"),
    );
    let expected_version = read_response["result"]["origins"]["gateway.originator"]["version"]
        .as_str()
        .expect("originator version")
        .to_string();
    let expected_file_path = read_response["result"]["origins"]["gateway.originator"]["name"]
        ["file"]
        .as_str()
        .expect("originator file")
        .to_string();

    let write_server = codexmanager_service::start_one_shot_server().expect("start server");
    let write_response = post_rpc(
        &write_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 109,
            method: "config/value/write".to_string(),
            params: Some(serde_json::json!({
                "keyPath": "gateway.originator",
                "value": "codex_cli_rs_custom",
                "mergeStrategy": "replace",
                "expectedVersion": expected_version,
            })),
        })
        .expect("serialize config write"),
    );
    let result = write_response.get("result").expect("config write result");
    assert_eq!(result["status"], "ok");
    assert_eq!(result["filePath"], expected_file_path);
    assert!(result["version"]
        .as_str()
        .is_some_and(|value| value.starts_with("sha256:")));

    let verify_server = codexmanager_service::start_one_shot_server().expect("start server");
    let verify_response = post_rpc(
        &verify_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 110,
            method: "config/read".to_string(),
            params: None,
        })
        .expect("serialize config verify read"),
    );
    assert_eq!(
        verify_response["result"]["config"]["gateway"]["originator"],
        "codex_cli_rs_custom"
    );
}

#[test]
fn rpc_config_batch_write_updates_multiple_gateway_fields() {
    let _ctx = RpcTestContext::new("rpc-config-batch-write");
    let server = codexmanager_service::start_one_shot_server().expect("start server");
    let response = post_rpc(
        &server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 111,
            method: "config/batchWrite".to_string(),
            params: Some(serde_json::json!({
                "edits": [
                    {
                        "keyPath": "gateway.route_strategy",
                        "value": "balanced",
                        "mergeStrategy": "replace"
                    },
                    {
                        "keyPath": "gateway.background_tasks.usage_refresh_workers",
                        "value": 6,
                        "mergeStrategy": "replace"
                    }
                ],
                "reloadUserConfig": true
            })),
        })
        .expect("serialize config batch write"),
    );
    let result = response.get("result").expect("config batch write result");
    assert_eq!(result["status"], "ok");

    let verify_server = codexmanager_service::start_one_shot_server().expect("start server");
    let verify_response = post_rpc(
        &verify_server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 112,
            method: "config/read".to_string(),
            params: None,
        })
        .expect("serialize config verify read"),
    );
    assert_eq!(
        verify_response["result"]["config"]["gateway"]["route_strategy"],
        "balanced"
    );
    assert_eq!(
        verify_response["result"]["config"]["gateway"]["background_tasks"]["usage_refresh_workers"],
        serde_json::json!(6)
    );
}

#[test]
fn rpc_config_value_write_reports_version_conflict() {
    let _ctx = RpcTestContext::new("rpc-config-version-conflict");
    let server = codexmanager_service::start_one_shot_server().expect("start server");
    let response = post_rpc(
        &server.addr,
        &serde_json::to_string(&JsonRpcRequest {
            id: 113,
            method: "config/value/write".to_string(),
            params: Some(serde_json::json!({
                "keyPath": "gateway.originator",
                "value": "conflict-originator",
                "mergeStrategy": "replace",
                "expectedVersion": "sha256:stale"
            })),
        })
        .expect("serialize config version conflict write"),
    );
    let result = response.get("result").expect("version conflict result");
    assert_eq!(
        result
            .get("configWriteErrorCode")
            .and_then(|value| value.as_str()),
        Some("configVersionConflict")
    );
    assert_eq!(
        result.get("errorCode").and_then(|value| value.as_str()),
        Some("invalid_request_payload")
    );
}

#[test]
fn rpc_account_list_empty_uses_default_pagination() {
    let _ctx = RpcTestContext::new("rpc-account-list-empty");
    let server = codexmanager_service::start_one_shot_server().expect("start server");

    let req = JsonRpcRequest {
        id: 2,
        method: "account/list".to_string(),
        params: None,
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let v = post_rpc(&server.addr, &json);
    let result = v.get("result").expect("result");

    let items = result
        .get("items")
        .and_then(|value| value.as_array())
        .expect("items array");
    assert!(items.is_empty(), "expected empty items, got: {result}");
    assert_eq!(
        result.get("total").and_then(|value| value.as_i64()),
        Some(0)
    );
    assert_eq!(result.get("page").and_then(|value| value.as_i64()), Some(1));
    assert_eq!(
        result.get("pageSize").and_then(|value| value.as_i64()),
        Some(5)
    );
}

#[test]
fn rpc_account_list_supports_pagination() {
    let ctx = RpcTestContext::new("rpc-account-list-page");
    ctx.seed_accounts(7);
    let server = codexmanager_service::start_one_shot_server().expect("start server");

    let req = JsonRpcRequest {
        id: 3,
        method: "account/list".to_string(),
        params: Some(serde_json::json!({"page": 2, "pageSize": 3})),
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let v = post_rpc(&server.addr, &json);
    let result = v.get("result").expect("result");

    let items = result
        .get("items")
        .and_then(|value| value.as_array())
        .expect("items array");
    assert_eq!(items.len(), 3, "unexpected page size: {result}");
    assert_eq!(
        result.get("total").and_then(|value| value.as_i64()),
        Some(7)
    );
    assert_eq!(result.get("page").and_then(|value| value.as_i64()), Some(2));
    assert_eq!(
        result.get("pageSize").and_then(|value| value.as_i64()),
        Some(3)
    );

    let ids = items
        .iter()
        .map(|value| {
            value
                .get("id")
                .and_then(|value| value.as_str())
                .expect("item id")
        })
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["acc-3", "acc-4", "acc-5"]);
    assert_eq!(
        items[0].get("status").and_then(|value| value.as_str()),
        Some("active")
    );
}

#[test]
fn rpc_app_settings_set_invalid_payload_returns_structured_error() {
    let _ctx = RpcTestContext::new("rpc-app-settings-invalid-payload");
    let server = codexmanager_service::start_one_shot_server().expect("start server");

    let req = JsonRpcRequest {
        id: 30,
        method: "appSettings/set".to_string(),
        params: Some(serde_json::json!("invalid-payload")),
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let v = post_rpc(&server.addr, &json);
    let result = v.get("result").expect("result");

    let message = result
        .get("error")
        .and_then(|value| value.as_str())
        .expect("error message");
    assert!(
        message.starts_with("invalid app settings payload:"),
        "unexpected message: {message}"
    );
    assert_eq!(
        result.get("errorCode").and_then(|value| value.as_str()),
        Some("invalid_settings_payload")
    );
    let detail = result.get("errorDetail").expect("errorDetail");
    assert_eq!(
        detail.get("code").and_then(|value| value.as_str()),
        Some("invalid_settings_payload")
    );
    assert_eq!(
        detail.get("message").and_then(|value| value.as_str()),
        Some(message)
    );
}

#[test]
fn rpc_app_settings_can_roundtrip_free_account_max_model() {
    let _ctx = RpcTestContext::new("rpc-app-settings-free-max-model");
    let set_server = codexmanager_service::start_one_shot_server().expect("start server");

    let set_req = JsonRpcRequest {
        id: 31,
        method: "appSettings/set".to_string(),
        params: Some(serde_json::json!({
            "freeAccountMaxModel": "gpt-5.3-codex"
        })),
    };
    let set_json = serde_json::to_string(&set_req).expect("serialize");
    let set_resp = post_rpc(&set_server.addr, &set_json);
    let set_result = set_resp.get("result").expect("result");
    assert_eq!(
        set_result
            .get("freeAccountMaxModel")
            .and_then(|value| value.as_str()),
        Some("gpt-5.3-codex")
    );

    let get_server = codexmanager_service::start_one_shot_server().expect("start server");
    let get_req = JsonRpcRequest {
        id: 32,
        method: "appSettings/get".to_string(),
        params: None,
    };
    let get_json = serde_json::to_string(&get_req).expect("serialize");
    let get_resp = post_rpc(&get_server.addr, &get_json);
    let get_result = get_resp.get("result").expect("result");
    assert_eq!(
        get_result
            .get("freeAccountMaxModel")
            .and_then(|value| value.as_str()),
        Some("gpt-5.3-codex")
    );
}

#[test]
fn rpc_account_list_active_filter_uses_backend_filtered_pagination() {
    let ctx = RpcTestContext::new("rpc-account-list-active-filter");
    let storage = Storage::open(ctx.db_path()).expect("open db");
    storage.init().expect("init schema");
    let now = now_ts();
    let accounts = [
        ("acc-active-1", "active", 0_i64, Some(20.0)),
        ("acc-active-2", "healthy", 1_i64, Some(30.0)),
        ("acc-low-1", "active", 2_i64, Some(85.0)),
        ("acc-inactive-1", "inactive", 3_i64, Some(10.0)),
        ("acc-no-snapshot", "active", 4_i64, None),
    ];
    for (id, status, sort, used_percent) in accounts {
        storage
            .insert_account(&Account {
                id: id.to_string(),
                label: id.to_string(),
                issuer: "https://auth.openai.com".to_string(),
                chatgpt_account_id: None,
                workspace_id: None,
                group_name: Some("group-a".to_string()),
                sort,
                status: status.to_string(),
                created_at: now + sort,
                updated_at: now + sort,
            })
            .expect("insert account");
        if let Some(used_percent) = used_percent {
            storage
                .insert_usage_snapshot(&UsageSnapshotRecord {
                    account_id: id.to_string(),
                    used_percent: Some(used_percent),
                    window_minutes: Some(300),
                    resets_at: None,
                    secondary_used_percent: None,
                    secondary_window_minutes: None,
                    secondary_resets_at: None,
                    credits_json: None,
                    captured_at: now + sort,
                })
                .expect("insert usage snapshot");
        }
    }

    let server = codexmanager_service::start_one_shot_server().expect("start server");
    let req = JsonRpcRequest {
        id: 30,
        method: "account/list".to_string(),
        params: Some(serde_json::json!({
            "page": 1,
            "pageSize": 2,
            "filter": "active",
            "groupFilter": "group-a"
        })),
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let v = post_rpc(&server.addr, &json);
    let result = v.get("result").expect("result");
    let items = result
        .get("items")
        .and_then(|value| value.as_array())
        .expect("items array");

    assert_eq!(items.len(), 2, "unexpected filtered page size: {result}");
    assert_eq!(
        result.get("total").and_then(|value| value.as_i64()),
        Some(3)
    );
    let ids = items
        .iter()
        .map(|value| {
            value
                .get("id")
                .and_then(|value| value.as_str())
                .expect("item id")
        })
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["acc-active-1", "acc-active-2"]);
}

#[test]
fn rpc_account_delete_many_deletes_requested_accounts() {
    let ctx = RpcTestContext::new("rpc-account-delete-many");
    ctx.seed_accounts(4);
    let server = codexmanager_service::start_one_shot_server().expect("start server");

    let req = JsonRpcRequest {
        id: 11,
        method: "account/deleteMany".to_string(),
        params: Some(serde_json::json!({
            "accountIds": ["acc-1", "acc-3", "missing"]
        })),
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let v = post_rpc(&server.addr, &json);
    let result = v.get("result").expect("result");

    assert_eq!(
        result.get("requested").and_then(|value| value.as_u64()),
        Some(3)
    );
    assert_eq!(
        result.get("deleted").and_then(|value| value.as_u64()),
        Some(2)
    );
    assert_eq!(
        result.get("failed").and_then(|value| value.as_u64()),
        Some(1)
    );
    let deleted = result
        .get("deletedAccountIds")
        .and_then(|value| value.as_array())
        .expect("deleted ids");
    assert_eq!(deleted.len(), 2);

    let storage = Storage::open(ctx.db_path()).expect("open db");
    let remaining = storage.list_accounts().expect("list remaining");
    let ids = remaining
        .into_iter()
        .map(|item| item.id)
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["acc-0", "acc-2"]);
}

#[test]
fn rpc_login_start_returns_url() {
    let _ctx = RpcTestContext::new("rpc-login-start");
    let server = codexmanager_service::start_one_shot_server().expect("start server");

    let req = JsonRpcRequest {
        id: 4,
        method: "account/login/start".to_string(),
        params: Some(serde_json::json!({"type": "chatgpt", "openBrowser": false})),
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let v = post_rpc(&server.addr, &json);
    let result = v.get("result").expect("result");
    let auth_url = result.get("authUrl").and_then(|v| v.as_str()).unwrap();
    let login_id = result.get("loginId").and_then(|v| v.as_str()).unwrap();
    assert!(auth_url.contains("oauth/authorize"));
    assert!(!login_id.is_empty());
}

#[test]
fn rpc_login_start_uses_default_oauth_values_when_env_is_blank() {
    let _ctx = RpcTestContext::new("rpc-login-start-default-oauth");
    let _issuer_guard = EnvGuard::set("CODEXMANAGER_ISSUER", "   ");
    let _client_id_guard = EnvGuard::set("CODEXMANAGER_CLIENT_ID", "");
    let server = codexmanager_service::start_one_shot_server().expect("start server");

    let req = JsonRpcRequest {
        id: 5,
        method: "account/login/start".to_string(),
        params: Some(serde_json::json!({"type": "chatgpt", "openBrowser": false})),
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let v = post_rpc(&server.addr, &json);
    let result = v.get("result").expect("result");
    let auth_url = result.get("authUrl").and_then(|v| v.as_str()).unwrap();
    let issuer = result.get("issuer").and_then(|v| v.as_str()).unwrap();
    let client_id = result.get("clientId").and_then(|v| v.as_str()).unwrap();

    assert!(auth_url.starts_with("https://auth.openai.com/oauth/authorize?"));
    assert!(auth_url.contains("client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
    assert_eq!(issuer, DEFAULT_ISSUER);
    assert_eq!(client_id, DEFAULT_CLIENT_ID);
}

#[test]
fn rpc_chatgpt_auth_tokens_login_read_logout_roundtrip() {
    let ctx = RpcTestContext::new("rpc-chatgpt-auth-tokens-roundtrip");
    let access_token = build_access_token(
        "sub-external",
        "embedded@example.com",
        "org-embedded",
        "pro",
    );

    let login_req = JsonRpcRequest {
        id: 41,
        method: "account/login/start".to_string(),
        params: Some(serde_json::json!({
            "type": "chatgptAuthTokens",
            "accessToken": access_token,
            "chatgptAccountId": "org-embedded",
            "chatgptPlanType": "pro"
        })),
    };
    let login_json = serde_json::to_string(&login_req).expect("serialize login");
    let login_server = codexmanager_service::start_one_shot_server().expect("start server");
    let login_resp = post_rpc(&login_server.addr, &login_json);
    let login_result = login_resp.get("result").expect("login result");
    let account_id = login_result
        .get("accountId")
        .and_then(|value| value.as_str())
        .expect("account id")
        .to_string();
    assert_eq!(
        login_result.get("type").and_then(|value| value.as_str()),
        Some("chatgptAuthTokens")
    );

    let read_req = JsonRpcRequest {
        id: 42,
        method: "account/read".to_string(),
        params: Some(serde_json::json!({ "refreshToken": false })),
    };
    let read_json = serde_json::to_string(&read_req).expect("serialize read");
    let read_server = codexmanager_service::start_one_shot_server().expect("start server");
    let read_resp = post_rpc(&read_server.addr, &read_json);
    let read_result = read_resp.get("result").expect("read result");
    let account = read_result.get("account").expect("current account");
    assert_eq!(
        read_result.get("authMode").and_then(|value| value.as_str()),
        Some("chatgpt")
    );
    assert_eq!(
        account.get("email").and_then(|value| value.as_str()),
        Some("embedded@example.com")
    );
    assert_eq!(
        account.get("planType").and_then(|value| value.as_str()),
        Some("pro")
    );
    assert_eq!(
        account
            .get("chatgptAccountId")
            .and_then(|value| value.as_str()),
        Some("org-embedded")
    );

    let logout_req = JsonRpcRequest {
        id: 43,
        method: "account/logout".to_string(),
        params: None,
    };
    let logout_json = serde_json::to_string(&logout_req).expect("serialize logout");
    let logout_server = codexmanager_service::start_one_shot_server().expect("start server");
    let logout_resp = post_rpc(&logout_server.addr, &logout_json);
    let logout_result = logout_resp.get("result").expect("logout result");
    assert_eq!(
        logout_result.get("ok").and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        logout_result
            .get("accountId")
            .and_then(|value| value.as_str()),
        Some(account_id.as_str())
    );

    let read_after_logout_server =
        codexmanager_service::start_one_shot_server().expect("start server");
    let read_after_logout = post_rpc(&read_after_logout_server.addr, &read_json);
    let read_after_logout_result = read_after_logout.get("result").expect("read result");
    assert!(read_after_logout_result.get("account").unwrap().is_null());

    let storage = Storage::open(ctx.db_path()).expect("open db");
    let account = storage
        .find_account_by_id(&account_id)
        .expect("find account")
        .expect("account exists");
    assert_eq!(account.status, "inactive");
}

#[test]
fn rpc_login_cancel_marks_pending_session_cancelled() {
    let _ctx = RpcTestContext::new("rpc-login-cancel");
    let start_server = codexmanager_service::start_one_shot_server().expect("start server");
    let start_req = JsonRpcRequest {
        id: 46,
        method: "account/login/start".to_string(),
        params: Some(serde_json::json!({"type": "chatgpt", "openBrowser": false})),
    };
    let start_json = serde_json::to_string(&start_req).expect("serialize login start");
    let start_resp = post_rpc(&start_server.addr, &start_json);
    let login_id = start_resp
        .get("result")
        .and_then(|value| value.get("loginId"))
        .and_then(|value| value.as_str())
        .expect("login id")
        .to_string();

    let cancel_server = codexmanager_service::start_one_shot_server().expect("start server");
    let cancel_req = JsonRpcRequest {
        id: 47,
        method: "account/login/cancel".to_string(),
        params: Some(serde_json::json!({ "loginId": login_id })),
    };
    let cancel_json = serde_json::to_string(&cancel_req).expect("serialize login cancel");
    let cancel_resp = post_rpc(&cancel_server.addr, &cancel_json);
    assert_eq!(cancel_resp.get("result"), Some(&serde_json::json!({})));
}

#[test]
fn rpc_account_rate_limits_read_returns_codex_snapshot() {
    let ctx = RpcTestContext::new("rpc-account-rate-limits");
    let access_token = build_access_token(
        "sub-rate-limits",
        "rate-limits@example.com",
        "org-rate-limits",
        "education",
    );

    let login_req = JsonRpcRequest {
        id: 48,
        method: "account/login/start".to_string(),
        params: Some(serde_json::json!({
            "type": "chatgptAuthTokens",
            "accessToken": access_token,
            "chatgptAccountId": "org-rate-limits",
            "chatgptPlanType": "education"
        })),
    };
    let login_json = serde_json::to_string(&login_req).expect("serialize login");
    let login_server = codexmanager_service::start_one_shot_server().expect("start server");
    let login_resp = post_rpc(&login_server.addr, &login_json);
    let account_id = login_resp
        .get("result")
        .and_then(|value| value.get("accountId"))
        .and_then(|value| value.as_str())
        .expect("account id")
        .to_string();

    let storage = Storage::open(ctx.db_path()).expect("open db");
    storage
        .insert_usage_snapshot(&UsageSnapshotRecord {
            account_id: account_id.clone(),
            used_percent: Some(12.0),
            window_minutes: Some(300),
            resets_at: Some(1770000000),
            secondary_used_percent: Some(55.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(1770600000),
            credits_json: Some(r#"{"remaining":123}"#.to_string()),
            captured_at: now_ts(),
        })
        .expect("insert usage snapshot");

    let read_server = codexmanager_service::start_one_shot_server().expect("start server");
    let read_req = JsonRpcRequest {
        id: 49,
        method: "account/rateLimits/read".to_string(),
        params: None,
    };
    let read_json = serde_json::to_string(&read_req).expect("serialize read");
    let read_resp = post_rpc(&read_server.addr, &read_json);
    let rate_limits = read_resp
        .get("result")
        .and_then(|value| value.get("rateLimits"))
        .expect("rate limits");

    assert_eq!(
        rate_limits.get("limitId").and_then(|value| value.as_str()),
        Some("codex")
    );
    assert_eq!(
        rate_limits.get("planType").and_then(|value| value.as_str()),
        Some("edu")
    );
    assert_eq!(
        rate_limits
            .get("primary")
            .and_then(|value| value.get("usedPercent"))
            .and_then(|value| value.as_i64()),
        Some(12)
    );
    assert_eq!(
        rate_limits
            .get("secondary")
            .and_then(|value| value.get("usedPercent"))
            .and_then(|value| value.as_i64()),
        Some(55)
    );
    assert_eq!(
        read_resp
            .get("result")
            .and_then(|value| value.get("rateLimitsByLimitId"))
            .and_then(|value| value.get("codex"))
            .and_then(|value| value.get("limitId"))
            .and_then(|value| value.as_str()),
        Some("codex")
    );
}

#[test]
fn rpc_chatgpt_auth_tokens_refresh_updates_access_token() {
    let _ctx = RpcTestContext::new("rpc-chatgpt-auth-tokens-refresh");
    let refreshed_access_token =
        build_access_token("sub-refresh", "refreshed@example.com", "org-refresh", "pro");
    let refresh_response = serde_json::json!({
        "access_token": refreshed_access_token,
        "refresh_token": "refresh-token-new"
    });
    let (issuer, refresh_rx, refresh_join) = start_mock_oauth_token_server(
        200,
        serde_json::to_string(&refresh_response).expect("serialize refresh response"),
    );
    let _issuer_guard = EnvGuard::set("CODEXMANAGER_ISSUER", &issuer);
    let _client_id_guard = EnvGuard::set("CODEXMANAGER_CLIENT_ID", "client-test-rpc-refresh");

    let login_req = JsonRpcRequest {
        id: 44,
        method: "account/login/start".to_string(),
        params: Some(serde_json::json!({
            "type": "chatgptAuthTokens",
            "accessToken": build_access_token(
                "sub-refresh",
                "initial@example.com",
                "org-refresh",
                "pro"
            ),
            "refreshToken": "refresh-token-old",
            "chatgptAccountId": "org-refresh"
        })),
    };
    let login_json = serde_json::to_string(&login_req).expect("serialize login");
    let login_server = codexmanager_service::start_one_shot_server().expect("start server");
    let login_resp = post_rpc(&login_server.addr, &login_json);
    let account_id = login_resp
        .get("result")
        .and_then(|value| value.get("accountId"))
        .and_then(|value| value.as_str())
        .expect("account id")
        .to_string();

    let refresh_req = JsonRpcRequest {
        id: 45,
        method: "account/chatgptAuthTokens/refresh".to_string(),
        params: Some(serde_json::json!({
            "reason": "unauthorized",
            "previousAccountId": "org-refresh"
        })),
    };
    let refresh_json = serde_json::to_string(&refresh_req).expect("serialize refresh");
    let refresh_server = codexmanager_service::start_one_shot_server().expect("start server");
    let refresh_rpc_resp = post_rpc(&refresh_server.addr, &refresh_json);
    let refresh_result = refresh_rpc_resp.get("result").expect("refresh result");
    assert_eq!(
        refresh_result
            .get("chatgptAccountId")
            .and_then(|value| value.as_str()),
        Some("org-refresh")
    );
    assert_eq!(
        refresh_result
            .get("accessToken")
            .and_then(|value| value.as_str()),
        Some(refreshed_access_token.as_str())
    );

    let refresh_body = refresh_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("receive refresh request");
    refresh_join.join().expect("join mock oauth server");
    assert!(refresh_body.contains("grant_type=refresh_token"));
    assert!(refresh_body.contains("refresh_token=refresh-token-old"));

    let storage =
        Storage::open(std::env::var("CODEXMANAGER_DB_PATH").expect("db path")).expect("open db");
    let token = storage
        .find_token_by_account_id(&account_id)
        .expect("find token")
        .expect("token exists");
    assert_eq!(token.access_token, refreshed_access_token);
    assert_eq!(token.refresh_token, "refresh-token-new");
}

#[test]
fn rpc_usage_read_empty() {
    let _ctx = RpcTestContext::new("rpc-usage-read");
    let server = codexmanager_service::start_one_shot_server().expect("start server");

    let req = JsonRpcRequest {
        id: 5,
        method: "account/usage/read".to_string(),
        params: None,
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let v = post_rpc(&server.addr, &json);
    let result = v.get("result").expect("result");
    assert!(result.get("snapshot").is_some());
}

#[test]
fn rpc_login_status_pending() {
    let _ctx = RpcTestContext::new("rpc-login-status");
    let server = codexmanager_service::start_one_shot_server().expect("start server");

    let req = JsonRpcRequest {
        id: 6,
        method: "account/login/status".to_string(),
        params: Some(serde_json::json!({"loginId": "login-1"})),
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let v = post_rpc(&server.addr, &json);
    let result = v.get("result").expect("result");
    assert!(result.get("status").is_some());
}

#[test]
fn rpc_usage_list_empty() {
    let _ctx = RpcTestContext::new("rpc-usage-list");
    let server = codexmanager_service::start_one_shot_server().expect("start server");

    let req = JsonRpcRequest {
        id: 7,
        method: "account/usage/list".to_string(),
        params: None,
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let v = post_rpc(&server.addr, &json);
    let result = v.get("result").expect("result");
    let items = result
        .get("items")
        .and_then(|value| value.as_array())
        .expect("items array");
    assert!(
        items.is_empty(),
        "expected empty usage items, got: {result}"
    );
}

#[test]
fn rpc_usage_aggregate_returns_backend_summary() {
    let ctx = RpcTestContext::new("rpc-usage-aggregate");
    let storage = Storage::open(ctx.db_path()).expect("open db");
    storage.init().expect("init schema");
    let now = now_ts();

    storage
        .insert_account(&Account {
            id: "acc-pro".to_string(),
            label: "Pro".to_string(),
            issuer: "https://auth.openai.com".to_string(),
            chatgpt_account_id: None,
            workspace_id: None,
            group_name: None,
            sort: 0,
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        })
        .expect("insert pro account");
    storage
        .insert_account(&Account {
            id: "acc-free".to_string(),
            label: "Free".to_string(),
            issuer: "https://auth.openai.com".to_string(),
            chatgpt_account_id: None,
            workspace_id: None,
            group_name: None,
            sort: 1,
            status: "active".to_string(),
            created_at: now + 1,
            updated_at: now + 1,
        })
        .expect("insert free account");

    storage
        .insert_usage_snapshot(&UsageSnapshotRecord {
            account_id: "acc-pro".to_string(),
            used_percent: Some(10.0),
            window_minutes: Some(300),
            resets_at: None,
            secondary_used_percent: Some(40.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: None,
            credits_json: None,
            captured_at: now,
        })
        .expect("insert pro usage");
    storage
        .insert_usage_snapshot(&UsageSnapshotRecord {
            account_id: "acc-free".to_string(),
            used_percent: Some(20.0),
            window_minutes: Some(10080),
            resets_at: None,
            secondary_used_percent: None,
            secondary_window_minutes: None,
            secondary_resets_at: None,
            credits_json: Some(r#"{"planType":"free"}"#.to_string()),
            captured_at: now,
        })
        .expect("insert free usage");

    let server = codexmanager_service::start_one_shot_server().expect("start server");
    let req = JsonRpcRequest {
        id: 71,
        method: "account/usage/aggregate".to_string(),
        params: None,
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let v = post_rpc(&server.addr, &json);
    let result = v.get("result").expect("result");

    assert_eq!(
        result
            .get("primaryBucketCount")
            .and_then(|value| value.as_i64()),
        Some(1)
    );
    assert_eq!(
        result
            .get("primaryRemainPercent")
            .and_then(|value| value.as_i64()),
        Some(90)
    );
    assert_eq!(
        result
            .get("secondaryBucketCount")
            .and_then(|value| value.as_i64()),
        Some(2)
    );
    assert_eq!(
        result
            .get("secondaryRemainPercent")
            .and_then(|value| value.as_i64()),
        Some(70)
    );
}

#[test]
fn rpc_requestlog_list_and_summary_support_pagination() {
    let ctx = RpcTestContext::new("rpc-requestlog-page");
    let storage = Storage::open(ctx.db_path()).expect("open db");
    storage.init().expect("init schema");

    for index in 0..4_i64 {
        let created_at = now_ts() + index;
        let status_code = if index < 2 { Some(200) } else { Some(502) };
        let request_log_id = storage
            .insert_request_log(&RequestLog {
                trace_id: Some(format!("trc-page-{index}")),
                key_id: Some("gk-page".to_string()),
                account_id: Some("acc-page".to_string()),
                initial_account_id: Some("acc-free".to_string()),
                attempted_account_ids_json: Some(r#"["acc-free","acc-page"]"#.to_string()),
                request_path: "/v1/responses".to_string(),
                original_path: Some("/v1/responses".to_string()),
                adapted_path: Some("/v1/responses".to_string()),
                method: "POST".to_string(),
                model: Some("gpt-5".to_string()),
                reasoning_effort: Some("medium".to_string()),
                response_adapter: Some("Passthrough".to_string()),
                upstream_url: Some("https://chatgpt.com/backend-api/codex/responses".to_string()),
                status_code,
                duration_ms: Some(500 + index),
                input_tokens: None,
                cached_input_tokens: None,
                output_tokens: None,
                total_tokens: None,
                reasoning_output_tokens: None,
                estimated_cost_usd: None,
                error: if status_code == Some(502) {
                    Some("stream interrupted".to_string())
                } else {
                    None
                },
                created_at,
            })
            .expect("insert request log");
        storage
            .insert_request_token_stat(&RequestTokenStat {
                request_log_id,
                key_id: Some("gk-page".to_string()),
                account_id: Some("acc-page".to_string()),
                model: Some("gpt-5".to_string()),
                input_tokens: Some(10),
                cached_input_tokens: Some(1),
                output_tokens: Some(2),
                total_tokens: Some(20 + index),
                reasoning_output_tokens: Some(0),
                estimated_cost_usd: Some(0.01),
                created_at,
            })
            .expect("insert token stat");
    }

    let server = codexmanager_service::start_one_shot_server().expect("start server");
    let list_req = JsonRpcRequest {
        id: 72,
        method: "requestlog/list".to_string(),
        params: Some(serde_json::json!({
            "page": 2,
            "pageSize": 1,
            "statusFilter": "5xx"
        })),
    };
    let list_json = serde_json::to_string(&list_req).expect("serialize requestlog list");
    let list_resp = post_rpc(&server.addr, &list_json);
    let list_result = list_resp.get("result").expect("requestlog list result");
    assert_eq!(
        list_result.get("total").and_then(|value| value.as_i64()),
        Some(2)
    );
    assert_eq!(
        list_result.get("page").and_then(|value| value.as_i64()),
        Some(2)
    );
    assert_eq!(
        list_result.get("pageSize").and_then(|value| value.as_i64()),
        Some(1)
    );
    let items = list_result
        .get("items")
        .and_then(|value| value.as_array())
        .expect("requestlog items");
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].get("traceId").and_then(|value| value.as_str()),
        Some("trc-page-2")
    );
    assert_eq!(
        items[0]
            .get("initialAccountId")
            .and_then(|value| value.as_str()),
        Some("acc-free")
    );
    assert_eq!(
        items[0]
            .get("attemptedAccountIds")
            .and_then(|value| value.as_array())
            .map(|items| items.len()),
        Some(2)
    );

    let summary_server = codexmanager_service::start_one_shot_server().expect("start server");
    let summary_req = JsonRpcRequest {
        id: 73,
        method: "requestlog/summary".to_string(),
        params: Some(serde_json::json!({
            "statusFilter": "5xx"
        })),
    };
    let summary_json = serde_json::to_string(&summary_req).expect("serialize requestlog summary");
    let summary_resp = post_rpc(&summary_server.addr, &summary_json);
    let summary_result = summary_resp
        .get("result")
        .expect("requestlog summary result");
    assert_eq!(
        summary_result
            .get("totalCount")
            .and_then(|value| value.as_i64()),
        Some(4)
    );
    assert_eq!(
        summary_result
            .get("filteredCount")
            .and_then(|value| value.as_i64()),
        Some(2)
    );
    assert_eq!(
        summary_result
            .get("errorCount")
            .and_then(|value| value.as_i64()),
        Some(2)
    );
    assert_eq!(
        summary_result
            .get("totalTokens")
            .and_then(|value| value.as_i64()),
        Some(45)
    );
}

#[test]
fn rpc_rejects_missing_token() {
    let _ctx = RpcTestContext::new("rpc-missing-token");
    let server = codexmanager_service::start_one_shot_server().expect("start server");

    let req = JsonRpcRequest {
        id: 8,
        method: "initialize".to_string(),
        params: None,
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let (status, _) = post_rpc_raw(&server.addr, &json, &[("Content-Type", "application/json")]);
    assert_eq!(status, 401);
}

#[test]
fn rpc_rejects_cross_site_origin() {
    let _ctx = RpcTestContext::new("rpc-cross-site-origin");
    let server = codexmanager_service::start_one_shot_server().expect("start server");

    let req = JsonRpcRequest {
        id: 9,
        method: "initialize".to_string(),
        params: None,
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let token = codexmanager_service::rpc_auth_token().to_string();
    let (status, _) = post_rpc_raw(
        &server.addr,
        &json,
        &[
            ("Content-Type", "application/json"),
            ("X-CodexManager-Rpc-Token", token.as_str()),
            ("Origin", "https://evil.example"),
            ("Sec-Fetch-Site", "cross-site"),
        ],
    );
    assert_eq!(status, 403);
}

#[test]
fn rpc_accepts_loopback_origin() {
    let _ctx = RpcTestContext::new("rpc-loopback-origin");
    let server = codexmanager_service::start_one_shot_server().expect("start server");

    let req = JsonRpcRequest {
        id: 10,
        method: "initialize".to_string(),
        params: None,
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let token = codexmanager_service::rpc_auth_token().to_string();
    let (status, body) = post_rpc_raw(
        &server.addr,
        &json,
        &[
            ("Content-Type", "application/json"),
            ("X-CodexManager-Rpc-Token", token.as_str()),
            ("Origin", "http://localhost:5173"),
            ("Sec-Fetch-Site", "same-site"),
        ],
    );
    assert_eq!(status, 200, "unexpected status {status}: {body}");
}
