use tiny_http::Request;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendRoute {
    Rpc,
    AuthCallback,
    Metrics,
    ServiceProbe,
    Gateway,
}

pub(crate) fn resolve_backend_route(method: &str, path: &str) -> BackendRoute {
    let path_only = path.split('?').next().unwrap_or(path);
    if method == "POST" && path_only == "/rpc" {
        return BackendRoute::Rpc;
    }
    if method == "GET" && path_only.starts_with("/auth/callback") {
        return BackendRoute::AuthCallback;
    }
    if method == "GET" && path_only == "/metrics" {
        return BackendRoute::Metrics;
    }
    if matches!(method, "GET" | "HEAD")
        && matches!(path_only, "/" | "/favicon.ico" | "/v1" | "/v1/")
    {
        return BackendRoute::ServiceProbe;
    }
    BackendRoute::Gateway
}

pub(crate) fn handle_backend_request(request: Request) {
    let route = resolve_backend_route(request.method().as_str(), request.url());
    match route {
        BackendRoute::Rpc => crate::http::rpc_endpoint::handle_rpc(request),
        BackendRoute::AuthCallback => crate::http::callback_endpoint::handle_callback(request),
        BackendRoute::Metrics => crate::http::gateway_endpoint::handle_metrics(request),
        BackendRoute::ServiceProbe => crate::http::gateway_endpoint::handle_service_probe(request),
        BackendRoute::Gateway => crate::http::gateway_endpoint::handle_gateway(request),
    }
}

#[cfg(test)]
#[path = "tests/backend_router_tests.rs"]
mod tests;
