use super::{
    http_queue_size, http_stream_queue_size, http_stream_worker_count, http_worker_count,
    panic_payload_message, start_backend_server, wake_backend_shutdown, HTTP_QUEUE_MIN,
    HTTP_STREAM_QUEUE_MIN, HTTP_STREAM_WORKER_MIN, HTTP_WORKER_MIN,
};
use std::sync::MutexGuard;

fn runtime_guard() -> MutexGuard<'static, ()> {
    crate::gateway::gateway_runtime_test_guard()
}

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
        crate::gateway::reload_runtime_config_from_env();
    }
}

#[test]
fn worker_count_has_minimum_guard() {
    assert!(http_worker_count() >= HTTP_WORKER_MIN);
    assert!(http_stream_worker_count() >= HTTP_STREAM_WORKER_MIN);
}

#[test]
fn queue_size_has_minimum_guard() {
    assert!(http_queue_size(0) >= HTTP_QUEUE_MIN);
    assert!(http_stream_queue_size(0) >= HTTP_STREAM_QUEUE_MIN);
}

#[test]
fn worker_and_queue_caps_apply_when_experimental_flag_is_enabled() {
    let _guard = runtime_guard();
    let _flag = EnvGuard::set("CODEXMANAGER_EXPERIMENTAL_CAPPED_HTTP_WORKERS", "1");
    let _worker_factor = EnvGuard::set("CODEXMANAGER_HTTP_WORKER_FACTOR", "64");
    let _worker_max = EnvGuard::set("CODEXMANAGER_HTTP_WORKER_MAX", "5");
    let _stream_worker_factor = EnvGuard::set("CODEXMANAGER_HTTP_STREAM_WORKER_FACTOR", "64");
    let _stream_worker_max = EnvGuard::set("CODEXMANAGER_HTTP_STREAM_WORKER_MAX", "3");
    let _queue_factor = EnvGuard::set("CODEXMANAGER_HTTP_QUEUE_FACTOR", "64");
    let _queue_max = EnvGuard::set("CODEXMANAGER_HTTP_QUEUE_MAX", "19");
    let _stream_queue_factor = EnvGuard::set("CODEXMANAGER_HTTP_STREAM_QUEUE_FACTOR", "64");
    let _stream_queue_max = EnvGuard::set("CODEXMANAGER_HTTP_STREAM_QUEUE_MAX", "11");
    crate::gateway::reload_runtime_config_from_env();

    assert_eq!(http_worker_count(), 8);
    assert_eq!(http_stream_worker_count(), 3);
    assert_eq!(http_queue_size(8), 32);
    assert_eq!(http_stream_queue_size(3), 16);
}

#[test]
fn panic_payload_message_formats_common_payloads() {
    let text = "boom";
    assert_eq!(panic_payload_message(&text), "boom");

    let owned = String::from("owned boom");
    assert_eq!(panic_payload_message(&owned), "owned boom");
}

#[test]
fn start_backend_server_reports_real_ephemeral_addr() {
    let backend = start_backend_server().expect("start backend");
    assert_ne!(backend.addr, "127.0.0.1:0");
    assert!(
        backend
            .addr
            .rsplit_once(':')
            .and_then(|(_, port)| port.parse::<u16>().ok())
            .is_some_and(|port| port != 0),
        "unexpected backend addr: {}",
        backend.addr
    );
    wake_backend_shutdown(&backend.addr);
    let _ = backend.join.join();
}
