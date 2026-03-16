use std::io;
use std::io::Write;
use std::net::TcpStream;
use std::panic::AssertUnwindSafe;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, SendTimeoutError, Sender};
use tiny_http::Request;
use tiny_http::Server;

const HTTP_WORKER_FACTOR: usize = 4;
const HTTP_WORKER_MIN: usize = 8;
const HTTP_STREAM_WORKER_FACTOR: usize = 1;
const HTTP_STREAM_WORKER_MIN: usize = 2;
const HTTP_QUEUE_FACTOR: usize = 4;
const HTTP_QUEUE_MIN: usize = 32;
const HTTP_STREAM_QUEUE_FACTOR: usize = 2;
const HTTP_STREAM_QUEUE_MIN: usize = 16;
const DEFAULT_HTTP_ENQUEUE_WAIT_TIMEOUT_MS: u64 = 25;
const ENV_HTTP_WORKER_FACTOR: &str = "CODEXMANAGER_HTTP_WORKER_FACTOR";
const ENV_HTTP_WORKER_MIN: &str = "CODEXMANAGER_HTTP_WORKER_MIN";
const ENV_HTTP_STREAM_WORKER_FACTOR: &str = "CODEXMANAGER_HTTP_STREAM_WORKER_FACTOR";
const ENV_HTTP_STREAM_WORKER_MIN: &str = "CODEXMANAGER_HTTP_STREAM_WORKER_MIN";
const ENV_HTTP_QUEUE_FACTOR: &str = "CODEXMANAGER_HTTP_QUEUE_FACTOR";
const ENV_HTTP_QUEUE_MIN: &str = "CODEXMANAGER_HTTP_QUEUE_MIN";
const ENV_HTTP_STREAM_QUEUE_FACTOR: &str = "CODEXMANAGER_HTTP_STREAM_QUEUE_FACTOR";
const ENV_HTTP_STREAM_QUEUE_MIN: &str = "CODEXMANAGER_HTTP_STREAM_QUEUE_MIN";
const ENV_HTTP_ENQUEUE_WAIT_TIMEOUT_MS: &str = "CODEXMANAGER_HTTP_ENQUEUE_WAIT_TIMEOUT_MS";

pub(crate) struct BackendServer {
    pub(crate) addr: String,
    pub(crate) join: thread::JoinHandle<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HttpQueueKind {
    Normal,
    Stream,
}

impl HttpQueueKind {
    fn is_stream(self) -> bool {
        matches!(self, Self::Stream)
    }
}

enum EnqueueError<T> {
    Overloaded(T, HttpQueueKind),
    Unavailable(T, HttpQueueKind),
}

fn http_worker_count() -> usize {
    // 中文注释：长流请求会占用处理线程；这里固定 worker 上限，避免并发时无限 spawn 拖垮进程。
    let cpus = thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4);
    let factor = env_usize_or(ENV_HTTP_WORKER_FACTOR, HTTP_WORKER_FACTOR).max(1);
    let min = env_usize_or(ENV_HTTP_WORKER_MIN, HTTP_WORKER_MIN).max(1);
    (cpus.saturating_mul(factor)).max(min)
}

fn http_stream_worker_count() -> usize {
    let cpus = thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4);
    let factor = env_usize_or(ENV_HTTP_STREAM_WORKER_FACTOR, HTTP_STREAM_WORKER_FACTOR).max(1);
    let min = env_usize_or(ENV_HTTP_STREAM_WORKER_MIN, HTTP_STREAM_WORKER_MIN).max(1);
    (cpus.saturating_mul(factor)).max(min)
}

fn http_queue_size(worker_count: usize) -> usize {
    // 中文注释：使用有界队列给入口施加背压；不这样做会在峰值流量下无限堆积请求并放大内存抖动。
    let factor = env_usize_or(ENV_HTTP_QUEUE_FACTOR, HTTP_QUEUE_FACTOR).max(1);
    let min = env_usize_or(ENV_HTTP_QUEUE_MIN, HTTP_QUEUE_MIN).max(1);
    worker_count.saturating_mul(factor).max(min)
}

fn http_stream_queue_size(worker_count: usize) -> usize {
    let factor = env_usize_or(ENV_HTTP_STREAM_QUEUE_FACTOR, HTTP_STREAM_QUEUE_FACTOR).max(1);
    let min = env_usize_or(ENV_HTTP_STREAM_QUEUE_MIN, HTTP_STREAM_QUEUE_MIN).max(1);
    worker_count.saturating_mul(factor).max(min)
}

fn env_usize_or(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_u64_or(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

fn http_enqueue_wait_timeout() -> Duration {
    Duration::from_millis(env_u64_or(
        ENV_HTTP_ENQUEUE_WAIT_TIMEOUT_MS,
        DEFAULT_HTTP_ENQUEUE_WAIT_TIMEOUT_MS,
    ))
}

fn spawn_request_workers(worker_count: usize, rx: Receiver<Request>, is_stream_queue: bool) {
    for _ in 0..worker_count {
        let worker_rx = rx.clone();
        let _ = thread::spawn(move || {
            while let Ok(request) = worker_rx.recv() {
                crate::gateway::record_http_queue_dequeue(is_stream_queue);
                handle_backend_request_safely(request);
            }
        });
    }
}

fn handle_backend_request_safely(request: Request) {
    let method = request.method().as_str().to_string();
    let path = request.url().to_string();
    if let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(|| {
        crate::http::backend_router::handle_backend_request(request);
    })) {
        log::error!(
            "backend request handler panicked: method={} path={} panic={}",
            method,
            path,
            panic_payload_message(payload.as_ref())
        );
    }
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic payload".to_string()
}

fn request_accept_header(request: &Request) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Accept"))
        .map(|header| header.value.as_str().to_ascii_lowercase())
}

fn request_is_stream_like(request: &Request) -> bool {
    request_accept_header(request).is_some_and(|value| value.contains("text/event-stream"))
}

fn enqueue_isolated<T>(
    item: T,
    queue_kind: HttpQueueKind,
    normal_tx: &Sender<T>,
    stream_tx: &Sender<T>,
    wait_timeout: Duration,
) -> Result<(), EnqueueError<T>> {
    let target = if queue_kind.is_stream() {
        stream_tx
    } else {
        normal_tx
    };
    match target.send_timeout(item, wait_timeout) {
        Ok(()) => Ok(()),
        Err(SendTimeoutError::Timeout(item)) => Err(EnqueueError::Overloaded(item, queue_kind)),
        Err(SendTimeoutError::Disconnected(item)) => {
            Err(EnqueueError::Unavailable(item, queue_kind))
        }
    }
}

fn enqueue_request(
    request: Request,
    normal_tx: &Sender<Request>,
    stream_tx: &Sender<Request>,
) -> Result<(), EnqueueError<Request>> {
    let queue_kind = if request_is_stream_like(&request) {
        HttpQueueKind::Stream
    } else {
        HttpQueueKind::Normal
    };
    enqueue_isolated(
        request,
        queue_kind,
        normal_tx,
        stream_tx,
        http_enqueue_wait_timeout(),
    )?;
    crate::gateway::record_http_queue_enqueue(queue_kind.is_stream());
    Ok(())
}

fn respond_queue_unavailable(request: Request, queue_kind: HttpQueueKind, overloaded: bool) {
    let message = if overloaded {
        if queue_kind.is_stream() {
            "gateway stream queue is saturated; retry this Codex stream shortly"
        } else {
            "gateway request queue is saturated; retry shortly"
        }
    } else if queue_kind.is_stream() {
        "gateway stream workers are unavailable"
    } else {
        "gateway request workers are unavailable"
    };

    let mut response = tiny_http::Response::from_string(format!(
        "{{\"error\":{{\"message\":\"{message}\",\"type\":\"server_error\",\"code\":\"gateway_queue_saturated\"}}}}"
    ))
    .with_status_code(503);
    if let Ok(content_type) = tiny_http::Header::from_bytes(
        b"Content-Type".as_slice(),
        b"application/json; charset=utf-8".as_slice(),
    ) {
        response.add_header(content_type);
    }
    if let Ok(retry_after) =
        tiny_http::Header::from_bytes(b"Retry-After".as_slice(), b"1".as_slice())
    {
        response.add_header(retry_after);
    }
    let _ = request.respond(response);
}

fn run_backend_server(server: Server) {
    let worker_count = http_worker_count();
    let stream_worker_count = http_stream_worker_count();
    let queue_size = http_queue_size(worker_count);
    let stream_queue_size = http_stream_queue_size(stream_worker_count);
    let (normal_tx, normal_rx) = bounded::<Request>(queue_size);
    let (stream_tx, stream_rx) = bounded::<Request>(stream_queue_size);
    crate::gateway::record_http_queue_capacity(queue_size, stream_queue_size);
    spawn_request_workers(worker_count, normal_rx, false);
    spawn_request_workers(stream_worker_count, stream_rx, true);

    for request in server.incoming_requests() {
        if crate::shutdown_requested() || request.url() == "/__shutdown" {
            let _ = request.respond(tiny_http::Response::from_string("shutdown"));
            break;
        }
        match enqueue_request(request, &normal_tx, &stream_tx) {
            Ok(()) => {}
            Err(EnqueueError::Overloaded(request, queue_kind)) => {
                crate::gateway::record_http_queue_enqueue_failure();
                respond_queue_unavailable(request, queue_kind, true);
            }
            Err(EnqueueError::Unavailable(request, queue_kind)) => {
                crate::gateway::record_http_queue_enqueue_failure();
                respond_queue_unavailable(request, queue_kind, false);
            }
        }
    }
}

pub(crate) fn start_backend_server() -> io::Result<BackendServer> {
    let server =
        Server::http("127.0.0.1:0").map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    let addr = server
        .server_addr()
        .to_ip()
        .map(|address| address.to_string())
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "backend addr missing"))?;
    let join = thread::spawn(move || run_backend_server(server));
    Ok(BackendServer { addr, join })
}

pub(crate) fn wake_backend_shutdown(addr: &str) {
    let Ok(mut stream) = TcpStream::connect(addr) else {
        return;
    };

    let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));

    let request = format!("GET /__shutdown HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    let _ = stream.write_all(request.as_bytes());
}

#[cfg(test)]
#[path = "tests/backend_runtime_tests.rs"]
mod tests;
