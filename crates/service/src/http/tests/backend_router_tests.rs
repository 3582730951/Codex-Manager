use super::{resolve_backend_route, BackendRoute};

#[test]
fn resolves_rpc_route() {
    assert_eq!(resolve_backend_route("POST", "/rpc"), BackendRoute::Rpc);
}

#[test]
fn resolves_auth_callback_route() {
    assert_eq!(
        resolve_backend_route("GET", "/auth/callback?code=123"),
        BackendRoute::AuthCallback
    );
}

#[test]
fn resolves_oauth_routes() {
    assert_eq!(
        resolve_backend_route("GET", "/oauth/authorize?client_id=abc"),
        BackendRoute::OAuthAuthorize
    );
    assert_eq!(
        resolve_backend_route("POST", "/oauth/authorize/approve"),
        BackendRoute::OAuthApprove
    );
    assert_eq!(
        resolve_backend_route("POST", "/oauth/token"),
        BackendRoute::OAuthToken
    );
    assert_eq!(
        resolve_backend_route("POST", "/api/accounts/deviceauth/usercode"),
        BackendRoute::DeviceUserCode
    );
    assert_eq!(
        resolve_backend_route("POST", "/api/accounts/deviceauth/token"),
        BackendRoute::DeviceToken
    );
    assert_eq!(
        resolve_backend_route("GET", "/codex/device"),
        BackendRoute::DeviceVerify
    );
}

#[test]
fn resolves_metrics_route() {
    assert_eq!(
        resolve_backend_route("GET", "/metrics"),
        BackendRoute::Metrics
    );
}

#[test]
fn falls_back_to_gateway_route() {
    assert_eq!(
        resolve_backend_route("POST", "/v1/responses"),
        BackendRoute::Gateway
    );
}
