use super::{
    http_queue_size, http_stream_queue_size, http_stream_worker_count, http_worker_count,
    panic_payload_message, start_backend_server, wake_backend_shutdown, HTTP_QUEUE_MIN,
    HTTP_STREAM_QUEUE_MIN, HTTP_STREAM_WORKER_MIN, HTTP_WORKER_MIN,
};

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
