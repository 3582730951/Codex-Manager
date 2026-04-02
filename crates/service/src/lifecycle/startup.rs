use std::io;
use std::panic;
use std::sync::OnceLock;
use std::thread;

pub struct ServerHandle {
    pub addr: String,
    join: Option<thread::JoinHandle<()>>,
    shutdown_on_drop: bool,
}

impl ServerHandle {
    pub fn join(mut self) {
        if self.shutdown_on_drop {
            crate::request_shutdown(&self.addr);
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        crate::clear_shutdown_flag();
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        if !self.shutdown_on_drop {
            return;
        }
        crate::request_shutdown(&self.addr);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        crate::clear_shutdown_flag();
    }
}

fn install_panic_hook() {
    static PANIC_HOOK_INSTALLED: OnceLock<()> = OnceLock::new();
    PANIC_HOOK_INSTALLED.get_or_init(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let current_thread = thread::current();
            let thread_name = current_thread.name().unwrap_or("unnamed");
            let location = info
                .location()
                .map(|value| format!("{}:{}", value.file(), value.line()))
                .unwrap_or_else(|| "unknown".to_string());
            let payload = if let Some(message) = info.payload().downcast_ref::<&str>() {
                (*message).to_string()
            } else if let Some(message) = info.payload().downcast_ref::<String>() {
                message.clone()
            } else {
                "unknown panic payload".to_string()
            };
            log::error!(
                "event=process_panic thread={} location={} payload={} backtrace_enabled={}",
                thread_name,
                location,
                payload,
                std::env::var_os("RUST_BACKTRACE").is_some()
            );
            previous(info);
        }));
    });
}

pub fn start_one_shot_server() -> std::io::Result<ServerHandle> {
    crate::clear_shutdown_flag();
    install_panic_hook();
    crate::portable::bootstrap_current_process();
    crate::gateway::reload_runtime_config_from_env();
    crate::gateway::cleanup_request_spool_dir();
    if let Err(err) = crate::storage_helpers::initialize_storage() {
        log::warn!("storage startup init skipped: {}", err);
    }
    crate::sync_runtime_settings_from_storage();
    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    let addr = server
        .server_addr()
        .to_ip()
        .map(|a| a.to_string())
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "server addr missing"))?;
    let join = thread::spawn(move || {
        if let Some(request) = server.incoming_requests().next() {
            crate::http::backend_router::handle_backend_request(request);
        }
    });
    Ok(ServerHandle {
        addr,
        join: Some(join),
        shutdown_on_drop: false,
    })
}

pub fn start_test_server() -> std::io::Result<ServerHandle> {
    crate::clear_shutdown_flag();
    install_panic_hook();
    crate::portable::bootstrap_current_process();
    crate::gateway::reload_runtime_config_from_env();
    crate::gateway::cleanup_request_spool_dir();
    if let Err(err) = crate::storage_helpers::initialize_storage() {
        log::warn!("storage startup init skipped: {}", err);
    }
    crate::sync_runtime_settings_from_storage();
    let backend = crate::http::backend_runtime::start_backend_server()?;
    Ok(ServerHandle {
        addr: backend.addr,
        join: Some(backend.join),
        shutdown_on_drop: true,
    })
}

pub fn start_server(addr: &str) -> std::io::Result<()> {
    install_panic_hook();
    crate::portable::bootstrap_current_process();
    crate::gateway::reload_runtime_config_from_env();
    crate::gateway::cleanup_request_spool_dir();
    if let Err(err) = crate::storage_helpers::initialize_storage() {
        log::warn!("storage startup init skipped: {}", err);
    }
    crate::sync_runtime_settings_from_storage();
    crate::usage_refresh::ensure_usage_polling();
    crate::usage_refresh::ensure_gateway_keepalive();
    crate::usage_refresh::ensure_token_refresh_polling();
    crate::http::server::start_http(addr)
}
