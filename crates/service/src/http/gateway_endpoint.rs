use tiny_http::{Header, Request, Response};

pub fn handle_gateway(request: Request) {
    if let Err(err) = crate::gateway::handle_gateway_request(request) {
        log::error!("gateway request error: {err}");
    }
}

pub fn handle_metrics(request: Request) {
    let body = crate::gateway::gateway_metrics_prometheus();
    let mut response = Response::from_string(body);
    if let Ok(content_type) = Header::from_bytes(b"Content-Type", b"text/plain; version=0.0.4") {
        response = response.with_header(content_type);
    }
    let _ = request.respond(response);
}

pub fn handle_service_probe(request: Request) {
    let path = request.url().split('?').next().unwrap_or(request.url());
    let (status, body, content_type) = match path {
        "/favicon.ico" => (204, String::new(), None),
        "/" => (
            200,
            r#"{"service":"codexmanager-service","message":"API service is running. Open the web UI on port 48761."}"#
                .to_string(),
            Some("application/json; charset=utf-8"),
        ),
        "/v1" | "/v1/" => (
            404,
            r#"{"error":"not_found","message":"Use a concrete /v1/* endpoint with Authorization, or open the web UI on port 48761."}"#
                .to_string(),
            Some("application/json; charset=utf-8"),
        ),
        _ => (
            404,
            r#"{"error":"not_found","message":"unsupported probe path"}"#.to_string(),
            Some("application/json; charset=utf-8"),
        ),
    };

    let mut response = Response::from_string(body).with_status_code(status);
    if let Some(value) = content_type {
        if let Ok(header) = Header::from_bytes(b"Content-Type", value.as_bytes()) {
            response = response.with_header(header);
        }
    }
    let _ = request.respond(response);
}
