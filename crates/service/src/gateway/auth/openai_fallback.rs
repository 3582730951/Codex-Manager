use bytes::Bytes;
use codexmanager_core::storage::{Account, Storage, Token};
use reqwest::blocking::Client;
use reqwest::Method;
use serde_json::Value;
use std::time::Instant;
use tiny_http::Request;

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
            let is_compaction_item = map
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("compaction"));
            let mut changed = map.remove("encrypted_content").is_some();
            let child_keys = map.keys().cloned().collect::<Vec<_>>();
            for key in child_keys {
                let mut remove_child = false;
                if let Some(child) = map.get_mut(&key) {
                    if strip_encrypted_content_value(child) {
                        changed = true;
                    }
                    remove_child = child.as_object().is_some_and(|obj| {
                        obj.get("type")
                            .and_then(Value::as_str)
                            .is_some_and(|value| value.eq_ignore_ascii_case("compaction"))
                            && !obj.contains_key("encrypted_content")
                    });
                }
                if remove_child {
                    map.remove(&key);
                }
            }
            if is_compaction_item {
                return true;
            }
            changed
        }
        Value::Array(items) => {
            let mut changed = false;
            let mut retained = Vec::with_capacity(items.len());
            for mut item in std::mem::take(items) {
                let item_changed = strip_encrypted_content_value(&mut item);
                let should_drop = item.as_object().is_some_and(|obj| {
                    obj.get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|value| value.eq_ignore_ascii_case("compaction"))
                        && !obj.contains_key("encrypted_content")
                });
                if item_changed {
                    changed = true;
                }
                if !should_drop {
                    retained.push(item);
                }
            }
            *items = retained;
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

fn is_compact_request_path(path: &str) -> bool {
    path == "/v1/responses/compact" || path.starts_with("/v1/responses/compact?")
}

pub(super) fn try_openai_fallback(
    client: &Client,
    storage: &Storage,
    method: &Method,
    request_path: &str,
    _request: &Request,
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

    let account_id = account
        .chatgpt_account_id
        .as_deref()
        .or_else(|| account.workspace_id.as_deref());
    let include_account_id = !compact_headers_mode && !is_openai_api_target;
    let mut upstream_headers = if is_compact_request_path(request_path) {
        let header_input = super::upstream::header_profile::CodexCompactUpstreamHeaderInput {
            auth_token: bearer.as_str(),
            account_id,
            include_account_id,
            upstream_cookie,
            incoming_session_id: incoming_headers.session_id(),
            incoming_subagent: incoming_headers.subagent(),
            fallback_session_id: None,
            strip_session_affinity,
            has_body: !body.is_empty(),
        };
        super::upstream::header_profile::build_codex_compact_upstream_headers(header_input)
    } else {
        let header_input = super::upstream::header_profile::CodexUpstreamHeaderInput {
            auth_token: bearer.as_str(),
            account_id,
            include_account_id,
            include_openai_beta: !compact_headers_mode
                && !is_openai_api_target
                && request_path.starts_with("/v1/responses"),
            upstream_cookie,
            incoming_session_id: incoming_headers.session_id(),
            incoming_client_request_id: incoming_headers.client_request_id(),
            incoming_subagent: incoming_headers.subagent(),
            fallback_session_id: None,
            incoming_turn_state: incoming_headers.turn_state(),
            include_turn_state: !compact_headers_mode && !is_openai_api_target,
            incoming_conversation_id: incoming_headers.conversation_id(),
            fallback_conversation_id: None,
            include_conversation_id: !compact_headers_mode && !is_openai_api_target,
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
