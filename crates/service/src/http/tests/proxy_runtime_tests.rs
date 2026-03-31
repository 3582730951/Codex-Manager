use super::{
    build_backend_base_url, build_internal_entity_headers, build_local_backend_client,
    proxy_handler_with_peer, ProxyState,
};
use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{Request as HttpRequest, StatusCode};
use reqwest::Client;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

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

    let response = runtime.block_on(proxy_handler_with_peer(
        State(state),
        SocketAddr::from(([127, 0, 0, 1], 34567)),
        request,
    ));
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

    let response = runtime.block_on(proxy_handler_with_peer(
        State(state),
        SocketAddr::from(([127, 0, 0, 1], 34567)),
        request,
    ));
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

#[test]
fn build_internal_entity_headers_uses_peer_runtime_mode() {
    let _mode = EnvGuard::set("CODEXMANAGER_CLIENT_ENTITY_MODE", "docker-peer-runtime");
    let _cidrs = EnvGuard::set("CODEXMANAGER_PEER_RUNTIME_TRUSTED_CIDRS", "172.18.0.0/16");
    crate::gateway::reload_runtime_config_from_env();

    let headers = build_internal_entity_headers(
        &axum::http::HeaderMap::new(),
        "POST",
        "/v1/responses",
        IpAddr::V4(Ipv4Addr::new(172, 18, 0, 8)),
    )
    .expect("internal peer headers");

    assert_eq!(headers.len(), 3);
    assert_eq!(
        headers[0].0,
        crate::gateway::affinity::INTERNAL_CLIENT_ENTITY_HEADER
    );
    assert_eq!(headers[0].1, "peerip:172.18.0.8");
}

#[test]
fn build_internal_entity_headers_skips_peer_runtime_when_forwarded_headers_exist() {
    let _mode = EnvGuard::set("CODEXMANAGER_CLIENT_ENTITY_MODE", "docker-peer-runtime");
    let _cidrs = EnvGuard::set("CODEXMANAGER_PEER_RUNTIME_TRUSTED_CIDRS", "172.18.0.0/16");
    crate::gateway::reload_runtime_config_from_env();

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "x-forwarded-for",
        "203.0.113.10".parse().expect("header value"),
    );
    let actual = build_internal_entity_headers(
        &headers,
        "POST",
        "/v1/responses",
        IpAddr::V4(Ipv4Addr::new(172, 18, 0, 8)),
    )
    .expect("internal peer headers");

    assert!(actual.is_empty());
}
