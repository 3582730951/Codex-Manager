use super::{build_backend_base_url, build_local_backend_client, proxy_handler, ProxyState};
use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{Request as HttpRequest, StatusCode};
use reqwest::Client;

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

#[test]
fn backend_base_url_uses_http_scheme() {
    assert_eq!(
        build_backend_base_url("127.0.0.1:18080"),
        "http://127.0.0.1:18080"
    );
}

#[test]
fn local_backend_client_builds_without_system_proxy() {
    build_local_backend_client().expect("local backend client");
}

#[test]
fn request_without_content_length_over_limit_returns_413() {
    let _guard = EnvGuard::set("CODEXMANAGER_FRONT_PROXY_MAX_BODY_BYTES", "8");
    crate::gateway::reload_runtime_config_from_env();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let state = ProxyState {
        backend_base_url: "http://127.0.0.1:1".to_string(),
        client: Client::new(),
    };
    let request = HttpRequest::builder()
        .method("POST")
        .uri("/rpc")
        .body(Body::from(vec![b'x'; 9]))
        .expect("request");

    let response = runtime.block_on(proxy_handler(State(state), request));
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = runtime
        .block_on(to_bytes(response.into_body(), usize::MAX))
        .expect("read body");
    let text = String::from_utf8(body.to_vec()).expect("utf8");
    assert!(text.contains("request body too large: content-length>8"));
}

#[test]
fn backend_send_failure_returns_502() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let state = ProxyState {
        backend_base_url: "http://127.0.0.1:1".to_string(),
        client: Client::new(),
    };
    let request = HttpRequest::builder()
        .method("GET")
        .uri("/rpc")
        .body(Body::empty())
        .expect("request");

    let response = runtime.block_on(proxy_handler(State(state), request));
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let error_code = response
        .headers()
        .get(crate::error_codes::ERROR_CODE_HEADER_NAME)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = runtime
        .block_on(to_bytes(response.into_body(), usize::MAX))
        .expect("read body");
    let text = String::from_utf8(body.to_vec()).expect("utf8");
    assert_eq!(error_code.as_deref(), Some("backend_proxy_error"));
    assert!(
        text.contains("backend proxy error:"),
        "unexpected body: {text}"
    );
}
