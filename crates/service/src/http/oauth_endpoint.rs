use tiny_http::Request;

pub fn handle_oauth_authorize(request: Request) {
    if let Err(err) = crate::auth::cli_oauth::handle_authorize_request(request) {
        log::warn!("oauth authorize request error: {err}");
    }
}

pub fn handle_oauth_approve(request: Request) {
    if let Err(err) = crate::auth::cli_oauth::handle_authorize_approve_request(request) {
        log::warn!("oauth authorize approve request error: {err}");
    }
}

pub fn handle_oauth_token(request: Request) {
    if let Err(err) = crate::auth::cli_oauth::handle_token_request(request) {
        log::warn!("oauth token request error: {err}");
    }
}

pub fn handle_device_usercode(request: Request) {
    if let Err(err) = crate::auth::cli_oauth::handle_device_usercode_request(request) {
        log::warn!("device usercode request error: {err}");
    }
}

pub fn handle_device_token(request: Request) {
    if let Err(err) = crate::auth::cli_oauth::handle_device_token_request(request) {
        log::warn!("device token request error: {err}");
    }
}

pub fn handle_device_verify(request: Request) {
    if let Err(err) = crate::auth::cli_oauth::handle_device_verify_request(request) {
        log::warn!("device verify request error: {err}");
    }
}
