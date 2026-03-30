use axum::http::{HeaderMap, HeaderName, HeaderValue, Uri};

use crate::http::header_filter::should_skip_request_header;

pub(crate) fn build_target_url(backend_base_url: &str, uri: &Uri) -> String {
    // 中文注释：部分 tiny_http 请求在重写后可能丢失 query；统一在这里拼接可避免多处实现不一致。
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    format!("{backend_base_url}{path_and_query}")
}

pub(crate) fn filter_request_headers(
    headers: &HeaderMap,
    strip_cli_affinity_id: bool,
    injected_headers: &[(&'static str, String)],
) -> HeaderMap {
    let mut outbound_headers = HeaderMap::new();
    for (name, value) in headers.iter() {
        if strip_cli_affinity_id && name.as_str().eq_ignore_ascii_case("x-codex-cli-affinity-id") {
            continue;
        }
        if should_skip_request_header(name, value) {
            continue;
        }
        let _ = outbound_headers.insert(name.clone(), value.clone());
    }
    for (name, value) in injected_headers {
        let Ok(header_name) = HeaderName::from_lowercase(name.as_bytes()) else {
            continue;
        };
        let Ok(header_value) = HeaderValue::from_str(value.as_str()) else {
            continue;
        };
        let _ = outbound_headers.insert(header_name, header_value);
    }
    outbound_headers
}

#[cfg(test)]
#[path = "tests/proxy_request_tests.rs"]
mod tests;
