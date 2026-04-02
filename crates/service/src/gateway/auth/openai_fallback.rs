use codexmanager_core::storage::{Account, Storage, Token};
use reqwest::blocking::Client;
use reqwest::Method;
use serde_json::Value;
use std::time::Instant;

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

fn strip_encrypted_content_from_payload(
    body: &super::RequestPayload,
) -> Result<Option<super::RequestPayload>, String> {
    let mut value = body.read_json_value()?;
    if !strip_encrypted_content_value(&mut value) {
        return Ok(None);
    }
    super::RequestPayload::from_json_value(&value).map(Some)
}

fn extract_prompt_cache_key(body: &super::RequestPayload) -> Option<String> {
    if body.is_empty() || body.len() > 64 * 1024 {
        return None;
    }
    let value = body.read_json_value().ok()?;
    value
        .get("prompt_cache_key")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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
    body: &super::RequestPayload,
    is_stream: bool,
    upstream_base: &str,
    account: &Account,
    token: &mut Token,
    strip_session_affinity: bool,
    debug: bool,
) -> Result<Option<reqwest::blocking::Response>, String> {
    let (url, _url_alt) = super::compute_upstream_url(upstream_base, request_path);
    let bearer = super::resolve_openai_bearer_token(storage, account, token)?;
    let attempt_started_at = Instant::now();
    let is_openai_api_target = super::is_openai_api_base(upstream_base);

    // `x-codex-turn-state` is an org-scoped encrypted blob. When we hit API-key fallback
    // (often a different org than the ChatGPT workspace), forwarding it can trigger:
    // `invalid_encrypted_content` / organization_id mismatch. In that case, prefer
    // resetting session affinity to keep the request usable.
    let strip_session_affinity =
        strip_session_affinity || incoming_headers.turn_state().is_some() || is_openai_api_target;
    let body_for_request =
        if strip_session_affinity && body.contains_bytes(br#""encrypted_content""#)? {
            strip_encrypted_content_from_payload(body)?.unwrap_or_else(|| body.clone())
        } else {
            body.clone()
        };
    let prompt_cache_key = if strip_session_affinity {
        None
    } else {
        extract_prompt_cache_key(&body_for_request)
    };
    let request_affinity = super::session_affinity::derive_outgoing_session_affinity(
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
    super::session_affinity::log_thread_anchor_conflict(
        request_path,
        account_id,
        incoming_headers.conversation_id(),
        prompt_cache_key.as_deref(),
    );
    let include_account_id = !is_openai_api_target;
    let mut upstream_headers = if is_compact_request_path(request_path) {
        let header_input = super::upstream::header_profile::CodexCompactUpstreamHeaderInput {
            auth_token: bearer.as_str(),
            account_id,
            include_account_id,
            incoming_session_id: request_affinity.incoming_session_id,
            incoming_subagent: incoming_headers.subagent(),
            fallback_session_id: request_affinity.fallback_session_id,
            strip_session_affinity,
            has_body: !body.is_empty(),
        };
        super::upstream::header_profile::build_codex_compact_upstream_headers(header_input)
    } else {
        let header_input = super::upstream::header_profile::CodexUpstreamHeaderInput {
            auth_token: bearer.as_str(),
            account_id,
            include_account_id,
            incoming_session_id: request_affinity.incoming_session_id,
            incoming_client_request_id: request_affinity.incoming_client_request_id,
            incoming_subagent: incoming_headers.subagent(),
            incoming_beta_features: incoming_headers.beta_features(),
            incoming_turn_metadata: incoming_headers.turn_metadata(),
            fallback_session_id: request_affinity.fallback_session_id,
            incoming_turn_state: request_affinity.incoming_turn_state,
            include_turn_state: !is_openai_api_target,
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
    let build_request = |http: &Client| -> Result<reqwest::blocking::RequestBuilder, String> {
        let mut builder = http.request(method.clone(), &url);
        for (name, value) in upstream_headers.iter() {
            builder = builder.header(name, value);
        }
        if !body_for_request.is_empty() {
            if body_for_request.is_file_backed() {
                let reader = body_for_request.open_blocking_reader()?;
                builder = builder.body(reqwest::blocking::Body::sized(
                    reader,
                    body_for_request.len() as u64,
                ));
            } else {
                builder = builder.body(body_for_request.read_all_bytes()?);
            }
        }
        Ok(builder)
    };
    let resp = match build_request(client)?.send() {
        Ok(resp) => resp,
        Err(first_err) => {
            let fresh = super::fresh_upstream_client_for_account(account.id.as_str());
            match build_request(&fresh)?.send() {
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
