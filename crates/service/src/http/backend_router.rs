use tiny_http::Request;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackendRoute {
    Rpc,
    AuthCallback,
    OAuthAuthorize,
    OAuthApprove,
    OAuthToken,
    DeviceUserCode,
    DeviceToken,
    DeviceVerify,
    Metrics,
    Gateway,
}

pub(crate) fn resolve_backend_route(method: &str, path: &str) -> BackendRoute {
    if method == "POST" && path == "/rpc" {
        return BackendRoute::Rpc;
    }
    if method == "GET" && path.starts_with("/auth/callback") {
        return BackendRoute::AuthCallback;
    }
    if method == "GET" && path.starts_with("/oauth/authorize") {
        return BackendRoute::OAuthAuthorize;
    }
    if method == "POST" && path == "/oauth/authorize/approve" {
        return BackendRoute::OAuthApprove;
    }
    if method == "POST" && path == "/oauth/token" {
        return BackendRoute::OAuthToken;
    }
    if method == "POST" && path == "/api/accounts/deviceauth/usercode" {
        return BackendRoute::DeviceUserCode;
    }
    if method == "POST" && path == "/api/accounts/deviceauth/token" {
        return BackendRoute::DeviceToken;
    }
    if method == "GET" && path.starts_with("/codex/device") {
        return BackendRoute::DeviceVerify;
    }
    if method == "GET" && path == "/metrics" {
        return BackendRoute::Metrics;
    }
    BackendRoute::Gateway
}

pub(crate) fn handle_backend_request(request: Request) {
    let route = resolve_backend_route(request.method().as_str(), request.url());
    match route {
        BackendRoute::Rpc => crate::http::rpc_endpoint::handle_rpc(request),
        BackendRoute::AuthCallback => crate::http::callback_endpoint::handle_callback(request),
        BackendRoute::OAuthAuthorize => {
            crate::http::oauth_endpoint::handle_oauth_authorize(request)
        }
        BackendRoute::OAuthApprove => crate::http::oauth_endpoint::handle_oauth_approve(request),
        BackendRoute::OAuthToken => crate::http::oauth_endpoint::handle_oauth_token(request),
        BackendRoute::DeviceUserCode => {
            crate::http::oauth_endpoint::handle_device_usercode(request)
        }
        BackendRoute::DeviceToken => crate::http::oauth_endpoint::handle_device_token(request),
        BackendRoute::DeviceVerify => crate::http::oauth_endpoint::handle_device_verify(request),
        BackendRoute::Metrics => crate::http::gateway_endpoint::handle_metrics(request),
        BackendRoute::Gateway => crate::http::gateway_endpoint::handle_gateway(request),
    }
}

#[cfg(test)]
#[path = "tests/backend_router_tests.rs"]
mod tests;
