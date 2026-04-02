use bytes::Bytes;
use codexmanager_core::storage::Account;
use std::time::Instant;
use tiny_http::Request;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestCompression {
    None,
    Zstd,
}

#[derive(Debug, Clone)]
enum PreparedRequestBody {
    Empty,
    Memory(Bytes),
    Payload(crate::gateway::RequestPayload),
}

#[derive(Debug)]
enum UpstreamSendError {
    Build(String),
    Transport(reqwest::Error),
}

impl std::fmt::Display for UpstreamSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Build(err) => write!(f, "{err}"),
            Self::Transport(err) => write!(f, "{err}"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(in super::super) struct UpstreamRequestContext<'a> {
    pub(in super::super) request_path: &'a str,
}

impl<'a> UpstreamRequestContext<'a> {
    pub(in super::super) fn from_request(request: &'a Request) -> Self {
        Self {
            request_path: request.url(),
        }
    }
}

fn should_force_connection_close(target_url: &str) -> bool {
    reqwest::Url::parse(target_url)
        .ok()
        .and_then(|url| url.host_str().map(|host| host.to_ascii_lowercase()))
        .is_some_and(|host| matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1"))
}

fn force_connection_close(headers: &mut Vec<(String, String)>) {
    if let Some((_, value)) = headers
        .iter_mut()
        .find(|(name, _)| name.eq_ignore_ascii_case("connection"))
    {
        *value = "close".to_string();
    } else {
        headers.push(("Connection".to_string(), "close".to_string()));
    }
}

fn extract_prompt_cache_key(body: &crate::gateway::RequestPayload) -> Option<String> {
    if body.is_empty() || body.len() > 64 * 1024 {
        return None;
    }
    let value = body.read_json_value().ok()?;
    value
        .get("prompt_cache_key")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn is_compact_request_path(path: &str) -> bool {
    path == "/v1/responses/compact" || path.starts_with("/v1/responses/compact?")
}

fn matches_gateway_account_proxy_request_target(
    target_url: &str,
    request_path: &str,
    is_stream: bool,
) -> bool {
    if !is_stream {
        return false;
    }
    if is_compact_request_path(request_path)
        || !(request_path == "/v1/responses" || request_path.starts_with("/v1/responses?"))
    {
        return false;
    }
    if !super::super::config::is_chatgpt_backend_base(target_url) {
        return false;
    }
    true
}

fn is_gateway_account_proxy_request(target_url: &str, request_path: &str, is_stream: bool) -> bool {
    if !matches_gateway_account_proxy_request_target(target_url, request_path, is_stream) {
        return false;
    }
    super::super::super::gateway_account_proxy_url().is_some()
}

fn is_gateway_account_proxy_connect_error(err: &reqwest::Error) -> bool {
    err.is_connect()
}

fn has_header(headers: &[(String, String)], name: &str) -> bool {
    headers
        .iter()
        .any(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
}

fn resolve_request_compression_with_flag(
    enabled: bool,
    target_url: &str,
    request_path: &str,
    is_stream: bool,
) -> RequestCompression {
    if !enabled {
        return RequestCompression::None;
    }
    if !is_stream {
        return RequestCompression::None;
    }
    if is_compact_request_path(request_path) || !request_path.starts_with("/v1/responses") {
        return RequestCompression::None;
    }
    if !super::super::config::is_chatgpt_backend_base(target_url) {
        return RequestCompression::None;
    }
    RequestCompression::Zstd
}

fn resolve_request_compression(
    target_url: &str,
    request_path: &str,
    is_stream: bool,
) -> RequestCompression {
    resolve_request_compression_with_flag(
        super::super::super::request_compression_enabled(),
        target_url,
        request_path,
        is_stream,
    )
}

fn encode_request_body(
    request_path: &str,
    body: &crate::gateway::RequestPayload,
    compression: RequestCompression,
    headers: &mut Vec<(String, String)>,
) -> Result<PreparedRequestBody, String> {
    if body.is_empty() || compression == RequestCompression::None {
        return Ok(if body.is_empty() {
            PreparedRequestBody::Empty
        } else {
            PreparedRequestBody::Payload(body.clone())
        });
    }
    if has_header(headers, "Content-Encoding") {
        log::warn!(
            "event=gateway_request_compression_skipped reason=content_encoding_exists path={}",
            request_path
        );
        return Ok(PreparedRequestBody::Payload(body.clone()));
    }
    if body.is_file_backed() {
        log::info!(
            "event=gateway_request_compression_skipped reason=file_backed_payload path={} bytes={}",
            request_path,
            body.len()
        );
        return Ok(PreparedRequestBody::Payload(body.clone()));
    }
    let compression_min_bytes = super::super::super::request_compression_min_bytes();
    if body.len() < compression_min_bytes {
        return Ok(PreparedRequestBody::Payload(body.clone()));
    }
    let body_bytes = body.read_all_bytes()?;
    match compression {
        RequestCompression::None => Ok(PreparedRequestBody::Payload(body.clone())),
        RequestCompression::Zstd => {
            match zstd::stream::encode_all(std::io::Cursor::new(body_bytes.as_ref()), 3) {
                Ok(compressed) => {
                    let post_bytes = compressed.len();
                    headers.push(("Content-Encoding".to_string(), "zstd".to_string()));
                    log::info!(
                    "event=gateway_request_compressed path={} algorithm=zstd pre_bytes={} post_bytes={}",
                    request_path,
                    body.len(),
                    post_bytes
                );
                    Ok(PreparedRequestBody::Memory(Bytes::from(compressed)))
                }
                Err(err) => {
                    log::warn!(
                        "event=gateway_request_compression_failed path={} algorithm=zstd err={}",
                        request_path,
                        err
                    );
                    Ok(PreparedRequestBody::Payload(body.clone()))
                }
            }
        }
    }
}

fn apply_prepared_body(
    builder: reqwest::blocking::RequestBuilder,
    body: &PreparedRequestBody,
) -> Result<reqwest::blocking::RequestBuilder, String> {
    match body {
        PreparedRequestBody::Empty => Ok(builder),
        PreparedRequestBody::Memory(bytes) => Ok(builder.body(bytes.clone())),
        PreparedRequestBody::Payload(payload) => {
            if payload.is_file_backed() {
                let reader = payload.open_blocking_reader()?;
                Ok(builder.body(reqwest::blocking::Body::sized(reader, payload.len() as u64)))
            } else {
                Ok(builder.body(payload.read_all_bytes()?))
            }
        }
    }
}

fn send_built_request(
    http: &reqwest::blocking::Client,
    method: &reqwest::Method,
    target_url: &str,
    request_deadline: Option<Instant>,
    is_stream: bool,
    headers: &[(String, String)],
    body: &PreparedRequestBody,
) -> Result<reqwest::blocking::Response, UpstreamSendError> {
    let mut builder = http.request(method.clone(), target_url);
    if let Some(timeout) =
        super::super::support::deadline::send_timeout(request_deadline, is_stream)
    {
        builder = builder.timeout(timeout);
    }
    for (name, value) in headers.iter() {
        builder = builder.header(name, value);
    }
    let builder = apply_prepared_body(builder, body).map_err(UpstreamSendError::Build)?;
    builder.send().map_err(UpstreamSendError::Transport)
}

pub(in super::super) fn send_upstream_request(
    client: &reqwest::blocking::Client,
    method: &reqwest::Method,
    target_url: &str,
    request_deadline: Option<Instant>,
    request_ctx: UpstreamRequestContext<'_>,
    incoming_headers: &super::super::super::IncomingHeaderSnapshot,
    body: &crate::gateway::RequestPayload,
    is_stream: bool,
    auth_token: &str,
    account: &Account,
    strip_session_affinity: bool,
) -> Result<reqwest::blocking::Response, String> {
    let attempt_started_at = Instant::now();
    let is_openai_api_target = super::super::super::is_openai_api_base(target_url);
    let prompt_cache_key = extract_prompt_cache_key(body);
    let is_compact_request = is_compact_request_path(request_ctx.request_path);
    let request_affinity = super::super::super::session_affinity::derive_outgoing_session_affinity(
        incoming_headers.session_id(),
        incoming_headers.client_request_id(),
        incoming_headers.turn_state(),
        incoming_headers.conversation_id(),
        prompt_cache_key.as_deref(),
    );
    let account_id = account
        .chatgpt_account_id
        .as_deref()
        .or_else(|| account.workspace_id.as_deref());
    super::super::super::session_affinity::log_thread_anchor_conflict(
        request_ctx.request_path,
        account_id,
        incoming_headers.conversation_id(),
        prompt_cache_key.as_deref(),
    );
    let include_account_id = !is_openai_api_target;
    let mut upstream_headers = if is_compact_request {
        let header_input = super::super::header_profile::CodexCompactUpstreamHeaderInput {
            auth_token,
            account_id,
            include_account_id,
            incoming_session_id: request_affinity.incoming_session_id,
            incoming_subagent: incoming_headers.subagent(),
            fallback_session_id: request_affinity.fallback_session_id,
            strip_session_affinity,
            has_body: !body.is_empty(),
        };
        super::super::header_profile::build_codex_compact_upstream_headers(header_input)
    } else {
        let header_input = super::super::header_profile::CodexUpstreamHeaderInput {
            auth_token,
            account_id,
            include_account_id,
            incoming_session_id: request_affinity.incoming_session_id,
            incoming_client_request_id: request_affinity.incoming_client_request_id,
            incoming_subagent: incoming_headers.subagent(),
            incoming_beta_features: incoming_headers.beta_features(),
            incoming_turn_metadata: incoming_headers.turn_metadata(),
            fallback_session_id: request_affinity.fallback_session_id,
            incoming_turn_state: request_affinity.incoming_turn_state,
            include_turn_state: true,
            strip_session_affinity,
            is_stream,
            has_body: !body.is_empty(),
        };
        super::super::header_profile::build_codex_upstream_headers(header_input)
    };
    if should_force_connection_close(target_url) {
        // 中文注释：本地 loopback mock/代理更容易复用到脏 keep-alive 连接；
        // 对 localhost/127.0.0.1 强制 close，避免请求落到已失效连接。
        force_connection_close(&mut upstream_headers);
    }
    let request_compression =
        resolve_request_compression(target_url, request_ctx.request_path, is_stream);
    let body_for_request = encode_request_body(
        request_ctx.request_path,
        body,
        request_compression,
        &mut upstream_headers,
    )?;

    let result =
        if is_gateway_account_proxy_request(target_url, request_ctx.request_path, is_stream) {
            if let Some(proxy_client) = super::super::super::gateway_account_proxy_client() {
                match send_built_request(
                    &proxy_client,
                    method,
                    target_url,
                    request_deadline,
                    is_stream,
                    upstream_headers.as_slice(),
                    &body_for_request,
                ) {
                    Ok(resp) => Ok(resp),
                    Err(UpstreamSendError::Transport(proxy_err))
                        if is_gateway_account_proxy_connect_error(&proxy_err) =>
                    {
                        log::warn!(
                        "event=gateway_account_proxy_fallback_direct path={} account_id={} err={}",
                        request_ctx.request_path,
                        account.id,
                        proxy_err
                    );
                        let direct = super::super::super::fresh_direct_upstream_client();
                        send_built_request(
                            &direct,
                            method,
                            target_url,
                            request_deadline,
                            is_stream,
                            upstream_headers.as_slice(),
                            &body_for_request,
                        )
                        .map_err(|err| err.to_string())
                    }
                    Err(proxy_err) => Err(proxy_err.to_string()),
                }
            } else {
                send_built_request(
                    client,
                    method,
                    target_url,
                    request_deadline,
                    is_stream,
                    upstream_headers.as_slice(),
                    &body_for_request,
                )
                .map_err(|err| err.to_string())
            }
        } else {
            match send_built_request(
                client,
                method,
                target_url,
                request_deadline,
                is_stream,
                upstream_headers.as_slice(),
                &body_for_request,
            ) {
                Ok(resp) => Ok(resp),
                Err(first_err) => {
                    // 中文注释：进程启动后才开启系统代理时，旧单例 client 可能仍走旧网络路径；
                    // 这里用 fresh client 立刻重试一次，避免必须手动重连服务。
                    let fresh =
                        super::super::super::fresh_upstream_client_for_account(account.id.as_str());
                    match send_built_request(
                        &fresh,
                        method,
                        target_url,
                        request_deadline,
                        is_stream,
                        upstream_headers.as_slice(),
                        &body_for_request,
                    ) {
                        Ok(resp) => Ok(resp),
                        Err(_) => Err(first_err.to_string()),
                    }
                }
            }
        };
    let duration_ms = super::super::super::duration_to_millis(attempt_started_at.elapsed());
    super::super::super::metrics::record_gateway_upstream_attempt(duration_ms, result.is_err());
    result
}

#[cfg(test)]
mod tests {
    use super::{
        encode_request_body, matches_gateway_account_proxy_request_target,
        resolve_request_compression_with_flag, RequestCompression,
    };
    use std::sync::MutexGuard;

    fn runtime_guard() -> MutexGuard<'static, ()> {
        crate::gateway::gateway_runtime_test_guard()
    }

    struct EnvGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.original {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
            crate::gateway::reload_runtime_config_from_env();
        }
    }

    #[test]
    fn request_compression_only_applies_to_streaming_chatgpt_responses() {
        assert_eq!(
            resolve_request_compression_with_flag(
                true,
                "https://chatgpt.com/backend-api/codex/responses",
                "/v1/responses",
                true
            ),
            RequestCompression::Zstd
        );
        assert_eq!(
            resolve_request_compression_with_flag(
                true,
                "https://chatgpt.com/backend-api/codex/responses",
                "/v1/responses/compact",
                true
            ),
            RequestCompression::None
        );
        assert_eq!(
            resolve_request_compression_with_flag(
                true,
                "https://api.openai.com/v1/responses",
                "/v1/responses",
                true
            ),
            RequestCompression::None
        );
        assert_eq!(
            resolve_request_compression_with_flag(
                true,
                "https://chatgpt.com/backend-api/codex/responses",
                "/v1/responses",
                false
            ),
            RequestCompression::None
        );
        assert_eq!(
            resolve_request_compression_with_flag(
                false,
                "https://chatgpt.com/backend-api/codex/responses",
                "/v1/responses",
                true
            ),
            RequestCompression::None
        );
    }

    #[test]
    fn encode_request_body_adds_zstd_content_encoding() {
        let _guard = runtime_guard();
        let _compression_min = EnvGuard::set("CODEXMANAGER_REQUEST_COMPRESSION_MIN_BYTES", "1");
        crate::gateway::reload_runtime_config_from_env();
        let body = crate::gateway::RequestPayload::from_vec(
            br#"{"model":"gpt-5.4","input":"compress me"}"#.to_vec(),
        )
        .expect("build request payload");
        let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];

        let actual = encode_request_body(
            "/v1/responses",
            &body,
            RequestCompression::Zstd,
            &mut headers,
        )
        .expect("encode request body");

        assert!(headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("Content-Encoding") && value == "zstd"
        }));
        let actual_bytes = match actual {
            super::PreparedRequestBody::Memory(bytes) => bytes,
            other => panic!("expected compressed in-memory body, got {other:?}"),
        };
        let decoded = zstd::stream::decode_all(std::io::Cursor::new(actual_bytes.as_ref()))
            .expect("decode zstd body");
        let value: serde_json::Value =
            serde_json::from_slice(&decoded).expect("parse decompressed json");
        assert_eq!(
            value.get("model").and_then(serde_json::Value::as_str),
            Some("gpt-5.4")
        );
    }

    #[test]
    fn encode_request_body_skips_compression_for_file_backed_payload() {
        let _guard = runtime_guard();
        let _spill = EnvGuard::set("CODEXMANAGER_REQUEST_SPILL_THRESHOLD_BYTES", "8");
        let _max = EnvGuard::set("CODEXMANAGER_FRONT_PROXY_MAX_BODY_BYTES", "1024");
        let _compression_min = EnvGuard::set("CODEXMANAGER_REQUEST_COMPRESSION_MIN_BYTES", "1");
        crate::gateway::reload_runtime_config_from_env();

        let body = crate::gateway::RequestPayload::from_vec(vec![b'x'; 64])
            .expect("build request payload");
        let mut headers = Vec::new();
        let actual = encode_request_body(
            "/v1/responses",
            &body,
            RequestCompression::Zstd,
            &mut headers,
        )
        .expect("encode request body");

        assert!(!headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("Content-Encoding")));
        match actual {
            super::PreparedRequestBody::Payload(payload) => assert!(payload.is_file_backed()),
            other => panic!("expected file-backed payload passthrough, got {other:?}"),
        }
    }

    #[test]
    fn encode_request_body_skips_compression_below_min_threshold() {
        let _guard = runtime_guard();
        let _spill = EnvGuard::set("CODEXMANAGER_REQUEST_SPILL_THRESHOLD_BYTES", "1024");
        let _max = EnvGuard::set("CODEXMANAGER_FRONT_PROXY_MAX_BODY_BYTES", "4096");
        let _compression_min = EnvGuard::set("CODEXMANAGER_REQUEST_COMPRESSION_MIN_BYTES", "1024");
        crate::gateway::reload_runtime_config_from_env();

        let body = crate::gateway::RequestPayload::from_vec(
            br#"{"model":"gpt-5.4","input":"tiny"}"#.to_vec(),
        )
        .expect("build request payload");
        let mut headers = Vec::new();
        let actual = encode_request_body(
            "/v1/responses",
            &body,
            RequestCompression::Zstd,
            &mut headers,
        )
        .expect("encode request body");

        assert!(headers
            .iter()
            .all(|(name, _)| !name.eq_ignore_ascii_case("Content-Encoding")));
        match actual {
            super::PreparedRequestBody::Payload(payload) => assert!(!payload.is_file_backed()),
            other => panic!("expected in-memory payload passthrough, got {other:?}"),
        }
    }

    #[test]
    fn gateway_account_proxy_only_applies_to_streaming_chatgpt_responses() {
        assert!(matches_gateway_account_proxy_request_target(
            "https://chatgpt.com/backend-api/codex/responses",
            "/v1/responses",
            true
        ));
        assert!(!matches_gateway_account_proxy_request_target(
            "https://chatgpt.com/backend-api/codex/responses",
            "/v1/responses",
            false
        ));
        assert!(!matches_gateway_account_proxy_request_target(
            "https://chatgpt.com/backend-api/codex/responses",
            "/v1/responses/compact",
            true
        ));
        assert!(!matches_gateway_account_proxy_request_target(
            "https://chatgpt.com/backend-api/codex/responses",
            "/v1/models",
            true
        ));
        assert!(!matches_gateway_account_proxy_request_target(
            "https://api.openai.com/v1/responses",
            "/v1/responses",
            true
        ));
    }
}
