use super::{
    build_callback_error_page, build_callback_success_page, ensure_login_server_with_addr,
    html_response, oauth_callback_error_message, resolve_redirect_uri, LOGIN_SERVER_STATE,
};
use crate::auth_login::login_start;
use std::net::TcpListener;
use std::sync::Mutex;
use url::Url;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn reset_login_server_state() {
    if let Some(cell) = LOGIN_SERVER_STATE.get() {
        if let Ok(mut guard) = cell.lock() {
            *guard = None;
        }
    }
}

#[test]
fn resolve_redirect_uri_prefers_login_server() {
    let _guard = ENV_LOCK.lock().expect("lock");
    reset_login_server_state();
    let prev_redirect = std::env::var("CODEXMANAGER_REDIRECT_URI").ok();
    let prev_login = std::env::var("CODEXMANAGER_LOGIN_ADDR").ok();
    let prev_service = std::env::var("CODEXMANAGER_SERVICE_ADDR").ok();

    std::env::remove_var("CODEXMANAGER_REDIRECT_URI");
    std::env::set_var("CODEXMANAGER_LOGIN_ADDR", "localhost:0");
    std::env::set_var("CODEXMANAGER_SERVICE_ADDR", "localhost:48760");

    let uri = resolve_redirect_uri().expect("redirect uri");
    let url = Url::parse(&uri).expect("parse redirect uri");
    assert_eq!(url.host_str(), Some("localhost"));
    let port = url.port_or_known_default().expect("port");
    assert_ne!(port, 48760);
    assert_eq!(url.path(), "/auth/callback");

    match prev_redirect {
        Some(value) => std::env::set_var("CODEXMANAGER_REDIRECT_URI", value),
        None => std::env::remove_var("CODEXMANAGER_REDIRECT_URI"),
    }
    match prev_login {
        Some(value) => std::env::set_var("CODEXMANAGER_LOGIN_ADDR", value),
        None => std::env::remove_var("CODEXMANAGER_LOGIN_ADDR"),
    }
    match prev_service {
        Some(value) => std::env::set_var("CODEXMANAGER_SERVICE_ADDR", value),
        None => std::env::remove_var("CODEXMANAGER_SERVICE_ADDR"),
    }
    reset_login_server_state();
}

#[test]
fn login_server_reports_port_in_use() {
    let _guard = ENV_LOCK.lock().expect("lock");
    reset_login_server_state();
    let prev_login = std::env::var("CODEXMANAGER_LOGIN_ADDR").ok();

    let listener_v6 = TcpListener::bind("[::1]:0").expect("bind v6 port");
    let port = listener_v6.local_addr().expect("addr").port();
    let listener_v4 = TcpListener::bind(format!("127.0.0.1:{port}")).ok();
    let err = match ensure_login_server_with_addr(&format!("localhost:{port}")) {
        Ok(_) => panic!("expected port in use error"),
        Err(err) => err,
    };
    assert!(err.contains("占用") || err.contains("in use"));

    drop(listener_v4);
    drop(listener_v6);
    match prev_login {
        Some(value) => std::env::set_var("CODEXMANAGER_LOGIN_ADDR", value),
        None => std::env::remove_var("CODEXMANAGER_LOGIN_ADDR"),
    }
    reset_login_server_state();
}

#[test]
fn login_server_rejects_non_loopback_by_default() {
    let _guard = ENV_LOCK.lock().expect("lock");
    reset_login_server_state();
    let prev_allow = std::env::var("CODEXMANAGER_ALLOW_NON_LOOPBACK_LOGIN_ADDR").ok();

    std::env::remove_var("CODEXMANAGER_ALLOW_NON_LOOPBACK_LOGIN_ADDR");
    let err = ensure_login_server_with_addr("0.0.0.0:1455").expect_err("should reject");
    assert!(err.contains("loopback"));

    match prev_allow {
        Some(value) => std::env::set_var("CODEXMANAGER_ALLOW_NON_LOOPBACK_LOGIN_ADDR", value),
        None => std::env::remove_var("CODEXMANAGER_ALLOW_NON_LOOPBACK_LOGIN_ADDR"),
    }
    reset_login_server_state();
}

#[test]
fn callback_success_page_contains_auto_close_script() {
    let html = build_callback_success_page();
    assert!(html.contains("window.close()"));
    assert!(html.contains("Login Success"));
    assert!(html.contains("Close Window"));
}

#[test]
fn callback_error_page_escapes_message() {
    let html = build_callback_error_page("bad <script>alert(1)</script>");
    assert!(html.contains("Login Failed"));
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
}

#[test]
fn callback_html_response_forces_connection_close() {
    let response = html_response(build_callback_success_page());
    let headers = response.headers();

    let content_type = headers
        .iter()
        .find(|header| header.field.equiv("Content-Type"))
        .map(|header| header.value.as_str());
    let connection = headers
        .iter()
        .find(|header| header.field.equiv("Connection"))
        .map(|header| header.value.as_str());

    assert_eq!(content_type, Some("text/html; charset=utf-8"));
    assert_eq!(connection, Some("close"));
}

#[test]
fn oauth_callback_error_message_maps_missing_entitlement() {
    let message = oauth_callback_error_message(
        "access_denied",
        Some("missing_codex_entitlement for workspace"),
    );
    assert!(message.contains("Codex is not enabled"));
}

#[test]
fn login_start_fails_when_login_server_cannot_bind() {
    let _guard = ENV_LOCK.lock().expect("lock");
    reset_login_server_state();
    let prev_login = std::env::var("CODEXMANAGER_LOGIN_ADDR").ok();

    std::env::set_var("CODEXMANAGER_LOGIN_ADDR", "0.0.0.0:1455");

    let err = login_start("browser", false, None, None, None, None).expect_err("should fail");
    assert!(err.contains("loopback"));

    match prev_login {
        Some(value) => std::env::set_var("CODEXMANAGER_LOGIN_ADDR", value),
        None => std::env::remove_var("CODEXMANAGER_LOGIN_ADDR"),
    }
    reset_login_server_state();
}
