use super::{
    build_backend_base_url, build_internal_entity_headers, build_local_backend_client,
    enforce_stream_body_limit, status_for_backend_proxy_error,
};
use axum::http::StatusCode;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::AtomicUsize;

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
fn streaming_body_limit_detects_overflow_without_content_length() {
    let seen = AtomicUsize::new(0);
    enforce_stream_body_limit(&seen, 4, 8).expect("first chunk should fit");
    enforce_stream_body_limit(&seen, 4, 8).expect("second chunk should fit");
    let err = enforce_stream_body_limit(&seen, 1, 8).expect_err("third chunk should overflow");

    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        err.to_string()
            .contains("request body too large: content-length>8"),
        "unexpected error: {err}"
    );
}

#[test]
fn backend_proxy_error_status_maps_too_large_to_413() {
    assert_eq!(
        status_for_backend_proxy_error(
            "backend proxy error: request body too large: content-length>8"
        ),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        status_for_backend_proxy_error("backend proxy error: tcp connect failed"),
        StatusCode::BAD_GATEWAY
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
