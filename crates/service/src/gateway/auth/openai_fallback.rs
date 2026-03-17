use bytes::Bytes;
use codexmanager_core::storage::{Account, Storage, Token};
use reqwest::blocking::Client;
use reqwest::Method;
use serde_json::Value;
use std::time::Instant;

struct RequestAffinityState<'a> {
    incoming_session_id: Option<&'a str>,
    incoming_client_request_id: Option<String>,
    incoming_turn_state: Option<&'a str>,
    fallback_session_id: Option<String>,
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

fn body_has_encrypted_content_hint(body: &[u8]) -> bool {
    // Fast path: avoid JSON parsing unless we hit the recovery path.
    std::str::from_utf8(body)
        .ok()
        .is_some_and(|text| text.contains("\"encrypted_content\""))
}

fn strip_encrypted_content_value(value: &mut Value) -> bool {
    match value {
        Value::Object(map) => {
            let mut changed = map.remove("encrypted_content").is_some();
            for v in map.values_mut() {
                if strip_encrypted_content_value(v) {
                    changed = true;
                }
            }
            changed
        }
        Value::Array(items) => {
            let mut changed = false;
            for item in items.iter_mut() {
                if strip_encrypted_content_value(item) {
                    changed = true;
                }
            }
            changed
        }
        _ => false,
    }
}

fn strip_encrypted_content_from_body(body: &[u8]) -> Option<Vec<u8>> {
    let mut value: Value = serde_json::from_slice(body).ok()?;
    if !strip_encrypted_content_value(&mut value) {
        return None;
    }
    serde_json::to_vec(&value).ok()
}

fn should_compact_upstream_headers() -> bool {
    super::cpa_no_cookie_header_mode_enabled()
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
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn resolve_request_affinity_state<'a>(
    incoming_session_id: Option<&'a str>,
    incoming_client_request_id: Option<&'a str>,
    incoming_turn_state: Option<&'a str>,
    conversation_id: Option<&'a str>,
    prompt_cache_key: Option<String>,
) -> RequestAffinityState<'a> {
    let original_incoming_session_id = incoming_session_id;
    let mut resolved_incoming_session_id = original_incoming_session_id;
    let resolved_client_request_id = incoming_client_request_id.map(str::to_string);
    let mut resolved_turn_state = incoming_turn_state;
    let conversation_anchor = conversation_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let effective_thread_anchor = prompt_cache_key
        .clone()
        .or_else(|| conversation_anchor.clone());

    if prompt_cache_key.is_some() {
        // 中文注释：当请求已携带线程锚点时，fallback 分支也应和主路径一样优先绑定到
        // 同一锚点，而不是继续复用旧 session_id。
        resolved_incoming_session_id = None;
    }
    if conversation_anchor.is_some() {
        // 中文注释：官方 ResponsesClient / CompactClient 都把 conversation_id 映射成
        // 上游 session_id。这里即使 prompt_cache_key 缺失，也让旧 session_id 退位，
        // 避免 compact 继续粘到历史兼容 session。
        resolved_incoming_session_id = None;
    }
    if resolved_turn_state.is_some()
        && original_incoming_session_id.is_none()
        && effective_thread_anchor.is_none()
    {
        // 中文注释：没有任何稳定线程锚点时，fallback 分支也不再信任孤立 turn-state，
        // 避免把 OpenAI fallback 粘回未知历史 turn。
        resolved_turn_state = None;
    }
    if let (Some(thread_anchor), Some(legacy_session_id)) = (
        effective_thread_anchor.as_deref(),
        original_incoming_session_id,
    ) {
        if legacy_session_id.trim() != thread_anchor {
            // 中文注释：旧 session_id 已被新的线程锚点覆盖时，继续透传旧 turn-state
            // 只会把 fallback 分支粘回历史 turn。
            resolved_turn_state = None;
        }
    }

    RequestAffinityState {
        incoming_session_id: resolved_incoming_session_id,
        incoming_client_request_id: resolved_client_request_id,
        incoming_turn_state: resolved_turn_state,
        fallback_session_id: effective_thread_anchor.clone(),
    }
}

fn is_compact_request_path(path: &str) -> bool {
    path == "/v1/responses/compact" || path.starts_with("/v1/responses/compact?")
}

pub(super) fn try_openai_fallback(
    client: &Client,
    storage: &Storage,
    method: &Method,
    request_path: &str,
    incoming_headers: &super::IncomingHeaderSnapshot,
    body: &Bytes,
    is_stream: bool,
    upstream_base: &str,
    account: &Account,
    token: &mut Token,
    upstream_cookie: Option<&str>,
    strip_session_affinity: bool,
    debug: bool,
) -> Result<Option<reqwest::blocking::Response>, String> {
    let (url, _url_alt) = super::compute_upstream_url(upstream_base, request_path);
    let bearer = super::resolve_openai_bearer_token(storage, account, token)?;
    let attempt_started_at = Instant::now();
    let compact_headers_mode = should_compact_upstream_headers();
    let is_openai_api_target = super::is_openai_api_base(upstream_base);
    let forwarded_upstream_cookie = if is_openai_api_target {
        None
    } else {
        upstream_cookie
    };

    // `x-codex-turn-state` is an org-scoped encrypted blob. When we hit API-key fallback
    // (often a different org than the ChatGPT workspace), forwarding it can trigger:
    // `invalid_encrypted_content` / organization_id mismatch. In that case, prefer
    // resetting session affinity to keep the request usable.
    let strip_session_affinity =
        strip_session_affinity || incoming_headers.turn_state().is_some() || is_openai_api_target;
    let body_for_request =
        if strip_session_affinity && body_has_encrypted_content_hint(body.as_ref()) {
            strip_encrypted_content_from_body(body.as_ref())
                .map(Bytes::from)
                .unwrap_or_else(|| body.clone())
        } else {
            body.clone()
        };
    let prompt_cache_key = if strip_session_affinity {
        None
    } else {
        extract_prompt_cache_key(body_for_request.as_ref())
    };
    let request_affinity = resolve_request_affinity_state(
        incoming_headers.session_id(),
        incoming_headers.client_request_id(),
        incoming_headers.turn_state(),
        incoming_headers.conversation_id(),
        prompt_cache_key,
    );

    let account_id = account
        .chatgpt_account_id
        .as_deref()
        .or_else(|| account.workspace_id.as_deref());
    let include_account_id = !is_openai_api_target;
    let mut upstream_headers = if is_compact_request_path(request_path) {
        let header_input = super::upstream::header_profile::CodexCompactUpstreamHeaderInput {
            auth_token: bearer.as_str(),
            account_id,
            include_account_id,
            upstream_cookie: forwarded_upstream_cookie,
            incoming_session_id: request_affinity.incoming_session_id,
            incoming_subagent: incoming_headers.subagent(),
            fallback_session_id: request_affinity.fallback_session_id.as_deref(),
            strip_session_affinity,
            has_body: !body.is_empty(),
        };
        super::upstream::header_profile::build_codex_compact_upstream_headers(header_input)
    } else {
        let header_input = super::upstream::header_profile::CodexUpstreamHeaderInput {
            auth_token: bearer.as_str(),
            account_id,
            include_account_id,
            upstream_cookie: forwarded_upstream_cookie,
            incoming_session_id: request_affinity.incoming_session_id,
            incoming_client_request_id: request_affinity.incoming_client_request_id.as_deref(),
            incoming_subagent: incoming_headers.subagent(),
            incoming_beta_features: incoming_headers.beta_features(),
            incoming_turn_metadata: incoming_headers.turn_metadata(),
            fallback_session_id: request_affinity.fallback_session_id.as_deref(),
            incoming_turn_state: request_affinity.incoming_turn_state,
            include_turn_state: !compact_headers_mode && !is_openai_api_target,
            strip_session_affinity,
            is_stream,
            has_body: !body.is_empty(),
        };
        super::upstream::header_profile::build_codex_upstream_headers(header_input)
    };
    if should_force_connection_close(&url) {
        force_connection_close(&mut upstream_headers);
    }
    if debug {
        log::debug!(
            "event=gateway_upstream_token_source path={} account_id={} token_source=api_key_access_token upstream_base={}",
            request_path,
            account_id.unwrap_or("-"),
            upstream_base
        );
    }
    let build_request = |http: &Client| {
        let mut builder = http.request(method.clone(), &url);
        for (name, value) in upstream_headers.iter() {
            builder = builder.header(name, value);
        }
        if !body_for_request.is_empty() {
            builder = builder.body(body_for_request.clone());
        }
        builder
    };
    let resp = match build_request(client).send() {
        Ok(resp) => resp,
        Err(first_err) => {
            let fresh = super::fresh_upstream_client_for_account(account.id.as_str());
            match build_request(&fresh).send() {
                Ok(resp) => resp,
                Err(second_err) => {
                    let duration_ms = super::duration_to_millis(attempt_started_at.elapsed());
                    super::metrics::record_gateway_upstream_attempt(duration_ms, true);
                    return Err(format!(
                        "{}; retry_after_fresh_client: {}",
                        first_err, second_err
                    ));
                }
            }
        }
    };
    let duration_ms = super::duration_to_millis(attempt_started_at.elapsed());
    super::metrics::record_gateway_upstream_attempt(duration_ms, false);
    Ok(Some(resp))
}

#[cfg(test)]
mod tests {
    use super::resolve_request_affinity_state;

    #[test]
    fn request_affinity_uses_thread_anchor_for_fallback_headers() {
        let actual = resolve_request_affinity_state(
            Some("legacy_session_should_not_win"),
            Some("legacy_request_id_should_not_win"),
            Some("legacy_turn_state_should_not_win"),
            Some("conv_anchor_fallback"),
            Some("conv_anchor_fallback".to_string()),
        );

        assert_eq!(actual.incoming_session_id, None);
        assert_eq!(
            actual.incoming_client_request_id.as_deref(),
            Some("legacy_request_id_should_not_win")
        );
        assert_eq!(actual.incoming_turn_state, None);
        assert_eq!(
            actual.fallback_session_id.as_deref(),
            Some("conv_anchor_fallback")
        );
    }

    #[test]
    fn request_affinity_keeps_plain_request_id_without_conversation_anchor() {
        let actual = resolve_request_affinity_state(
            None,
            Some("explicit_client_request_id"),
            Some("turn_state_ok"),
            None,
            None,
        );

        assert_eq!(actual.incoming_session_id, None);
        assert_eq!(
            actual.incoming_client_request_id.as_deref(),
            Some("explicit_client_request_id")
        );
        assert_eq!(actual.incoming_turn_state, Some("turn_state_ok"));
        assert_eq!(actual.fallback_session_id, None);
    }

    #[test]
    fn request_affinity_drops_turn_state_without_thread_anchor() {
        let actual =
            resolve_request_affinity_state(None, None, Some("orphan_turn_state"), None, None);

        assert_eq!(actual.incoming_session_id, None);
        assert_eq!(actual.incoming_client_request_id, None);
        assert_eq!(actual.incoming_turn_state, None);
        assert_eq!(actual.fallback_session_id, None);
    }

    #[test]
    fn request_affinity_uses_conversation_anchor_when_prompt_cache_missing() {
        let actual = resolve_request_affinity_state(
            Some("legacy_session_should_not_win"),
            Some("legacy_request_id_should_not_win"),
            Some("legacy_turn_state_should_not_win"),
            Some("conv_anchor_only"),
            None,
        );

        assert_eq!(actual.incoming_session_id, None);
        assert_eq!(
            actual.incoming_client_request_id.as_deref(),
            Some("legacy_request_id_should_not_win")
        );
        assert_eq!(actual.incoming_turn_state, None);
        assert_eq!(
            actual.fallback_session_id.as_deref(),
            Some("conv_anchor_only")
        );
    }
}
