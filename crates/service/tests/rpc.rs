use codexmanager_core::rpc::types::JsonRpcRequest;
use codexmanager_core::storage::{now_ts, Account, Storage, UsageSnapshotRecord};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

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
    let mut stream = TcpStream::connect(addr).expect("connect server");
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
