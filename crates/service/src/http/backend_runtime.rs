use std::io;
use std::io::Write;
use std::net::TcpStream;
use std::panic::AssertUnwindSafe;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender};
use tiny_http::Request;
use tiny_http::Server;

const HTTP_WORKER_FACTOR: usize = 4;
const HTTP_WORKER_MIN: usize = 8;
const HTTP_WORKER_MAX: usize = 16;
const HTTP_STREAM_WORKER_FACTOR: usize = 1;
const HTTP_STREAM_WORKER_MIN: usize = 2;
const HTTP_STREAM_WORKER_MAX: usize = 4;
const HTTP_QUEUE_FACTOR: usize = 4;
const HTTP_QUEUE_MIN: usize = 32;
const HTTP_QUEUE_MAX: usize = 128;
const HTTP_STREAM_QUEUE_FACTOR: usize = 2;
const HTTP_STREAM_QUEUE_MIN: usize = 16;
const HTTP_STREAM_QUEUE_MAX: usize = 32;
const ENV_HTTP_WORKER_FACTOR: &str = "CODEXMANAGER_HTTP_WORKER_FACTOR";
const ENV_HTTP_WORKER_MIN: &str = "CODEXMANAGER_HTTP_WORKER_MIN";
const ENV_HTTP_WORKER_MAX: &str = "CODEXMANAGER_HTTP_WORKER_MAX";
const ENV_HTTP_STREAM_WORKER_FACTOR: &str = "CODEXMANAGER_HTTP_STREAM_WORKER_FACTOR";
const ENV_HTTP_STREAM_WORKER_MIN: &str = "CODEXMANAGER_HTTP_STREAM_WORKER_MIN";
const ENV_HTTP_STREAM_WORKER_MAX: &str = "CODEXMANAGER_HTTP_STREAM_WORKER_MAX";
const ENV_HTTP_QUEUE_FACTOR: &str = "CODEXMANAGER_HTTP_QUEUE_FACTOR";
const ENV_HTTP_QUEUE_MIN: &str = "CODEXMANAGER_HTTP_QUEUE_MIN";
const ENV_HTTP_QUEUE_MAX: &str = "CODEXMANAGER_HTTP_QUEUE_MAX";
const ENV_HTTP_STREAM_QUEUE_FACTOR: &str = "CODEXMANAGER_HTTP_STREAM_QUEUE_FACTOR";
const ENV_HTTP_STREAM_QUEUE_MIN: &str = "CODEXMANAGER_HTTP_STREAM_QUEUE_MIN";
const ENV_HTTP_STREAM_QUEUE_MAX: &str = "CODEXMANAGER_HTTP_STREAM_QUEUE_MAX";

pub(crate) struct BackendServer {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) addr: String,
    pub(crate) join: thread::JoinHandle<()>,
}

fn localhost_bind_variants(bind_addr: &str) -> Option<[String; 2]> {
    let trimmed = bind_addr.trim();
    if trimmed.len() <= "localhost:".len()
        || !trimmed[..("localhost:".len())].eq_ignore_ascii_case("localhost:")
    {
        return None;
    }
    let port = &trimmed["localhost:".len()..];
    Some([format!("127.0.0.1:{port}"), format!("[::1]:{port}")])
}

fn http_worker_count() -> usize {
    // 中文注释：长流请求会占用处理线程；这里固定 worker 上限，避免并发时无限 spawn 拖垮进程。
    let cpus = thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4);
    let factor = env_usize_or(ENV_HTTP_WORKER_FACTOR, HTTP_WORKER_FACTOR).max(1);
    let min = env_usize_or(ENV_HTTP_WORKER_MIN, HTTP_WORKER_MIN).max(1);
    let workers = (cpus.saturating_mul(factor)).max(min);
    if crate::gateway::experimental_capped_http_workers_enabled() {
        let max = env_usize_or(ENV_HTTP_WORKER_MAX, HTTP_WORKER_MAX).max(min);
        workers.min(max)
    } else {
        workers
    }
}

fn http_stream_worker_count() -> usize {
    let cpus = thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4);
    let factor = env_usize_or(ENV_HTTP_STREAM_WORKER_FACTOR, HTTP_STREAM_WORKER_FACTOR).max(1);
    let min = env_usize_or(ENV_HTTP_STREAM_WORKER_MIN, HTTP_STREAM_WORKER_MIN).max(1);
    let workers = (cpus.saturating_mul(factor)).max(min);
    if crate::gateway::experimental_capped_http_workers_enabled() {
        let max = env_usize_or(ENV_HTTP_STREAM_WORKER_MAX, HTTP_STREAM_WORKER_MAX).max(min);
        workers.min(max)
    } else {
        workers
    }
}

fn http_queue_size(worker_count: usize) -> usize {
    // 中文注释：使用有界队列给入口施加背压；不这样做会在峰值流量下无限堆积请求并放大内存抖动。
    let factor = env_usize_or(ENV_HTTP_QUEUE_FACTOR, HTTP_QUEUE_FACTOR).max(1);
    let min = env_usize_or(ENV_HTTP_QUEUE_MIN, HTTP_QUEUE_MIN).max(1);
    let size = worker_count.saturating_mul(factor).max(min);
    if crate::gateway::experimental_capped_http_workers_enabled() {
        let max = env_usize_or(ENV_HTTP_QUEUE_MAX, HTTP_QUEUE_MAX).max(min);
        size.min(max)
    } else {
        size
    }
}

fn http_stream_queue_size(worker_count: usize) -> usize {
    let factor = env_usize_or(ENV_HTTP_STREAM_QUEUE_FACTOR, HTTP_STREAM_QUEUE_FACTOR).max(1);
    let min = env_usize_or(ENV_HTTP_STREAM_QUEUE_MIN, HTTP_STREAM_QUEUE_MIN).max(1);
    let size = worker_count.saturating_mul(factor).max(min);
    if crate::gateway::experimental_capped_http_workers_enabled() {
        let max = env_usize_or(ENV_HTTP_STREAM_QUEUE_MAX, HTTP_STREAM_QUEUE_MAX).max(min);
        size.min(max)
    } else {
        size
    }
}

fn env_usize_or(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default)
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

fn enqueue_request(
    request: Request,
    normal_tx: &Sender<Request>,
    stream_tx: &Sender<Request>,
) -> Result<(), ()> {
    let prefer_stream = request_is_stream_like(&request);
    if prefer_stream {
        match stream_tx.send(request) {
            Ok(()) => {
                crate::gateway::record_http_queue_enqueue(true);
                Ok(())
            }
            Err(err) => {
                normal_tx.send(err.into_inner()).map_err(|_| ())?;
                crate::gateway::record_http_queue_enqueue(false);
                Ok(())
            }
        }
    } else {
        match normal_tx.send(request) {
            Ok(()) => {
                crate::gateway::record_http_queue_enqueue(false);
                Ok(())
            }
            Err(err) => {
                stream_tx.send(err.into_inner()).map_err(|_| ())?;
                crate::gateway::record_http_queue_enqueue(true);
                Ok(())
            }
        }
    }
}

fn run_backend_accept_loop(server: Server, normal_tx: Sender<Request>, stream_tx: Sender<Request>) {
    for request in server.incoming_requests() {
        if crate::shutdown_requested() || request.url() == "/__shutdown" {
            let _ = request.respond(tiny_http::Response::from_string("shutdown"));
            break;
        }
        if enqueue_request(request, &normal_tx, &stream_tx).is_err() {
            crate::gateway::record_http_queue_enqueue_failure();
            break;
        }
    }
}

fn run_backend_servers(servers: Vec<Server>) {
    let worker_count = http_worker_count();
    let stream_worker_count = http_stream_worker_count();
    let queue_size = http_queue_size(worker_count);
    let stream_queue_size = http_stream_queue_size(stream_worker_count);
    let (normal_tx, normal_rx) = bounded::<Request>(queue_size);
    let (stream_tx, stream_rx) = bounded::<Request>(stream_queue_size);
    crate::gateway::record_http_queue_capacity(queue_size, stream_queue_size);
    spawn_request_workers(worker_count, normal_rx, false);
    spawn_request_workers(stream_worker_count, stream_rx, true);
    let acceptors = servers
        .into_iter()
        .map(|server| {
            let normal_tx = normal_tx.clone();
            let stream_tx = stream_tx.clone();
            thread::spawn(move || run_backend_accept_loop(server, normal_tx, stream_tx))
        })
        .collect::<Vec<_>>();
    drop(normal_tx);
    drop(stream_tx);
    for join in acceptors {
        let _ = join.join();
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn start_backend_server() -> io::Result<BackendServer> {
    start_backend_server_at("127.0.0.1:0")
}

pub(crate) fn start_backend_server_at(bind_addr: &str) -> io::Result<BackendServer> {
    let (servers, addr) = if let Some([v4, v6]) = localhost_bind_variants(bind_addr) {
        let v4_server = Server::http(&v4).map_err(|err| io::Error::new(io::ErrorKind::Other, err));
        let v6_server = Server::http(&v6).map_err(|err| io::Error::new(io::ErrorKind::Other, err));
        let servers = match (v4_server, v6_server) {
            (Ok(v4_server), Ok(v6_server)) => vec![v4_server, v6_server],
            (Ok(server), Err(_)) | (Err(_), Ok(server)) => vec![server],
            (Err(err), Err(_)) => return Err(err),
        };
        (servers, bind_addr.trim().to_string())
    } else {
        let server =
            Server::http(bind_addr).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        let addr = server
            .server_addr()
            .to_ip()
            .map(|value| value.to_string())
            .unwrap_or_else(|| bind_addr.trim().to_string());
        (vec![server], addr)
    };
    let join = thread::spawn(move || run_backend_servers(servers));
    Ok(BackendServer { addr, join })
}

#[cfg_attr(not(test), allow(dead_code))]
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
