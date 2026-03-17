use bytes::Bytes;
use codexmanager_core::storage::Account;
use std::time::Instant;
use tiny_http::Request;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestCompression {
    None,
    Zstd,
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

fn extract_prompt_cache_key(body: &[u8]) -> Option<String> {
    if body.is_empty() || body.len() > 64 * 1024 {
        return None;
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return None;
    };
    value
        .get("prompt_cache_key")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn should_compact_upstream_headers() -> bool {
    super::super::super::cpa_no_cookie_header_mode_enabled()
}

fn is_compact_request_path(path: &str) -> bool {
    path == "/v1/responses/compact" || path.starts_with("/v1/responses/compact?")
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
    body: &Bytes,
    compression: RequestCompression,
    headers: &mut Vec<(String, String)>,
) -> Bytes {
    if body.is_empty() || compression == RequestCompression::None {
        return body.clone();
    }
    if has_header(headers, "Content-Encoding") {
        log::warn!(
            "event=gateway_request_compression_skipped reason=content_encoding_exists path={}",
            request_path
        );
        return body.clone();
    }
    match compression {
        RequestCompression::None => body.clone(),
        RequestCompression::Zstd => {
            match zstd::stream::encode_all(std::io::Cursor::new(body.as_ref()), 3) {
                Ok(compressed) => {
                    let post_bytes = compressed.len();
                    headers.push(("Content-Encoding".to_string(), "zstd".to_string()));
                    log::info!(
                    "event=gateway_request_compressed path={} algorithm=zstd pre_bytes={} post_bytes={}",
                    request_path,
                    body.len(),
                    post_bytes
                );
                    Bytes::from(compressed)
                }
                Err(err) => {
                    log::warn!(
                        "event=gateway_request_compression_failed path={} algorithm=zstd err={}",
                        request_path,
                        err
                    );
                    body.clone()
                }
            }
        }
    }
}

pub(in super::super) fn send_upstream_request(
    client: &reqwest::blocking::Client,
    method: &reqwest::Method,
    target_url: &str,
    request_deadline: Option<Instant>,
    request_ctx: UpstreamRequestContext<'_>,
    incoming_headers: &super::super::super::IncomingHeaderSnapshot,
    body: &Bytes,
    is_stream: bool,
    upstream_cookie: Option<&str>,
    auth_token: &str,
    account: &Account,
    strip_session_affinity: bool,
) -> Result<reqwest::blocking::Response, reqwest::Error> {
    let attempt_started_at = Instant::now();
    let compact_headers_mode = should_compact_upstream_headers();
    let is_openai_api_target = super::super::super::is_openai_api_base(target_url);
    let prompt_cache_key = if strip_session_affinity {
        None
    } else {
        extract_prompt_cache_key(body.as_ref())
    };
    let compact_conversation_anchor = if strip_session_affinity {
        None
    } else {
        incoming_headers
            .conversation_id()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let original_incoming_session_id = incoming_headers.session_id();
    let mut incoming_session_id = original_incoming_session_id;
    let incoming_client_request_id = incoming_headers.client_request_id();
    let mut incoming_turn_state = incoming_headers.turn_state();
    if prompt_cache_key.is_some() {
        // 中文注释：当请求已携带线程锚点（prompt_cache_key）时，优先让上游会话头也绑定到
        // 同一锚点，避免继续透传旧 session_id 造成线程漂移或跨账号粘性。
        incoming_session_id = None;
    }
    if is_compact_request_path(request_ctx.request_path) && compact_conversation_anchor.is_some() {
        // 中文注释：官方 compact 客户端会直接把 conversation_id 映射成 session_id。
        // compact 请求没有 prompt_cache_key，因此这里显式让会话头退回到 conversation 锚点。
        incoming_session_id = None;
    }
    if incoming_turn_state.is_some()
        && original_incoming_session_id.is_none()
        && prompt_cache_key.is_none()
    {
        // 中文注释：客户端单独塞一个 turn-state、却没有任何稳定线程锚点时，
        // 这份状态无法证明属于当前请求线程。此时直接透传只会把上游粘到未知历史 turn。
        incoming_turn_state = None;
    }
    if let (Some(cache_key), Some(legacy_session_id)) =
        (prompt_cache_key.as_deref(), original_incoming_session_id)
    {
        if legacy_session_id.trim() != cache_key {
            // 中文注释：旧 session_id 已被新的线程锚点覆盖时，继续透传旧 turn-state
            // 只会把上游路由粘到历史 turn，和官方同 turn 回放语义相悖。
            incoming_turn_state = None;
        }
    }
    let mut derived_session_id = if !strip_session_affinity && incoming_session_id.is_none() {
        // 中文注释：这里继续保留“基于账号/密钥的稳定 session”兼容策略，
        // 但不再把 remote 地址混进会话锚点，避免同一客户端仅因源地址变化就得到不同 session_id，
        // 让主路径更接近官方可见的线程锚点语义。
        super::super::header_profile::derive_sticky_session_id_from_headers(incoming_headers)
    } else {
        None
    };
    // 中文注释：当 prompt_cache_key 存在时，用它对齐请求会话锚点。
    // 官方可见线程锚点仍以 prompt_cache_key + session_id 为主，不额外默认补 x-client-request-id。
    if !strip_session_affinity {
        if let Some(cache_key) = prompt_cache_key.as_ref() {
            derived_session_id = Some(cache_key.clone());
        }
    }
    let compact_fallback_session_id = compact_conversation_anchor
        .as_deref()
        .or(derived_session_id.as_deref());
    let account_id = account
        .chatgpt_account_id
        .as_deref()
        .or_else(|| account.workspace_id.as_deref());
    let include_account_id = !is_openai_api_target;
    let forwarded_upstream_cookie = if is_openai_api_target {
        None
    } else {
        upstream_cookie
    };
    let mut upstream_headers = if is_compact_request_path(request_ctx.request_path) {
        let header_input = super::super::header_profile::CodexCompactUpstreamHeaderInput {
            auth_token,
            account_id,
            include_account_id,
            upstream_cookie: forwarded_upstream_cookie,
            incoming_session_id,
            incoming_subagent: incoming_headers.subagent(),
            fallback_session_id: compact_fallback_session_id,
            strip_session_affinity,
            has_body: !body.is_empty(),
        };
        super::super::header_profile::build_codex_compact_upstream_headers(header_input)
    } else {
        let header_input = super::super::header_profile::CodexUpstreamHeaderInput {
            auth_token,
            account_id,
            include_account_id,
            upstream_cookie: forwarded_upstream_cookie,
            incoming_session_id,
            incoming_client_request_id,
            incoming_subagent: incoming_headers.subagent(),
            incoming_beta_features: incoming_headers.beta_features(),
            incoming_turn_metadata: incoming_headers.turn_metadata(),
            fallback_session_id: derived_session_id.as_deref(),
            incoming_turn_state,
            include_turn_state: !compact_headers_mode,
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
    );
    let build_request = |http: &reqwest::blocking::Client| {
        let mut builder = http.request(method.clone(), target_url);
        if let Some(timeout) =
            super::super::support::deadline::send_timeout(request_deadline, is_stream)
        {
            builder = builder.timeout(timeout);
        }
        for (name, value) in upstream_headers.iter() {
            builder = builder.header(name, value);
        }
        if !body_for_request.is_empty() {
            builder = builder.body(body_for_request.clone());
        }
        builder
    };

    let result = match build_request(client).send() {
        Ok(resp) => Ok(resp),
        Err(first_err) => {
            // 中文注释：进程启动后才开启系统代理时，旧单例 client 可能仍走旧网络路径；
            // 这里用 fresh client 立刻重试一次，避免必须手动重连服务。
            let fresh = super::super::super::fresh_upstream_client_for_account(account.id.as_str());
            match build_request(&fresh).send() {
                Ok(resp) => Ok(resp),
                Err(_) => Err(first_err),
            }
        }
    };
    let duration_ms = super::super::super::duration_to_millis(attempt_started_at.elapsed());
    super::super::super::metrics::record_gateway_upstream_attempt(duration_ms, result.is_err());
    result
}

#[cfg(test)]
mod tests {
    use super::{encode_request_body, resolve_request_compression_with_flag, RequestCompression};
    use bytes::Bytes;

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
        let body = Bytes::from_static(br#"{"model":"gpt-5.4","input":"compress me"}"#);
        let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];

        let actual = encode_request_body(
            "/v1/responses",
            &body,
            RequestCompression::Zstd,
            &mut headers,
        );

        assert!(headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("Content-Encoding") && value == "zstd"
        }));
        let decoded = zstd::stream::decode_all(std::io::Cursor::new(actual.as_ref()))
            .expect("decode zstd body");
        let value: serde_json::Value =
            serde_json::from_slice(&decoded).expect("parse decompressed json");
        assert_eq!(
            value.get("model").and_then(serde_json::Value::as_str),
            Some("gpt-5.4")
        );
    }
}
