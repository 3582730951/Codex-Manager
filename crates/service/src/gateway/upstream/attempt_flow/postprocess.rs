use bytes::Bytes;
use codexmanager_core::storage::{Account, Storage, Token};
use std::time::Instant;

use crate::account_status::mark_account_unavailable_for_refresh_token_error;

use super::super::support::backoff;
use super::super::support::outcome::{decide_upstream_outcome, UpstreamOutcomeDecision};
use super::super::support::retry::{retry_with_alternate_path, AltPathRetryResult};
use super::fallback_branch::{handle_openai_fallback_branch, FallbackBranchResult};
use super::stateless_retry::{retry_stateless_then_optional_alt, StatelessRetryResult};
use super::transport::UpstreamRequestContext;

fn try_refresh_chatgpt_access_token(
    storage: &Storage,
    upstream_base: &str,
    account: &Account,
    token: &mut Token,
) -> Result<Option<String>, String> {
    if super::super::super::is_openai_api_base(upstream_base) {
        return Ok(None);
    }
    if token.refresh_token.trim().is_empty() {
        return Ok(None);
    }
    let issuer = if account.issuer.trim().is_empty() {
        super::super::super::runtime_config::token_exchange_default_issuer()
    } else {
        account.issuer.clone()
    };
    let client_id = super::super::super::runtime_config::token_exchange_client_id();
    crate::usage_token_refresh::refresh_and_persist_access_token(
        storage,
        token,
        issuer.as_str(),
        client_id.as_str(),
    )?;
    let refreshed = token.access_token.trim();
    if refreshed.is_empty() {
        return Err("refreshed chatgpt access token is empty".to_string());
    }
    Ok(Some(refreshed.to_string()))
}

#[allow(clippy::too_many_arguments)]
fn retry_upstream_server_error_once(
    client: &reqwest::blocking::Client,
    method: &reqwest::Method,
    url: &str,
    request_deadline: Option<Instant>,
    request_ctx: UpstreamRequestContext<'_>,
    incoming_headers: &super::super::super::IncomingHeaderSnapshot,
    body: &Bytes,
    is_stream: bool,
    auth_token: &str,
    account: &Account,
    strip_session_affinity: bool,
    debug: bool,
    status: reqwest::StatusCode,
) -> Result<Option<reqwest::blocking::Response>, ()> {
    if status.as_u16() != 500 {
        return Ok(None);
    }
    if debug {
        log::warn!(
            "event=gateway_upstream_server_error_retry path={} status={} account_id={}",
            request_ctx.request_path,
            status.as_u16(),
            account.id
        );
    }
    if !backoff::sleep_with_exponential_jitter(
        std::time::Duration::from_millis(120),
        std::time::Duration::from_millis(900),
        1,
        request_deadline,
    ) {
        return Err(());
    }

    match super::transport::send_upstream_request(
        client,
        method,
        url,
        request_deadline,
        request_ctx,
        incoming_headers,
        body,
        is_stream,
        auth_token,
        account,
        strip_session_affinity,
    ) {
        Ok(resp) => Ok(Some(resp)),
        Err(err) => {
            log::warn!(
                "event=gateway_upstream_server_error_retry_error path={} status=502 account_id={} err={}",
                request_ctx.request_path,
                account.id,
                err
            );
            Ok(None)
        }
    }
}

pub(super) enum PostRetryFlowDecision {
    Failover,
    Terminal { status_code: u16, message: String },
    RespondUpstream(reqwest::blocking::Response),
}

#[allow(clippy::too_many_arguments)]
pub(super) fn process_upstream_post_retry_flow<F>(
    client: &reqwest::blocking::Client,
    storage: &Storage,
    method: &reqwest::Method,
    upstream_base: &str,
    path: &str,
    url: &str,
    url_alt: Option<&str>,
    request_deadline: Option<Instant>,
    request_ctx: UpstreamRequestContext<'_>,
    incoming_headers: &super::super::super::IncomingHeaderSnapshot,
    body: &Bytes,
    is_stream: bool,
    auth_token: &str,
    account: &Account,
    token: &mut Token,
    upstream_fallback_base: Option<&str>,
    strip_session_affinity: bool,
    debug: bool,
    allow_openai_fallback: bool,
    disable_challenge_stateless_retry: bool,
    has_more_candidates: bool,
    mut upstream: reqwest::blocking::Response,
    mut log_gateway_result: F,
) -> PostRetryFlowDecision
where
    F: FnMut(Option<&str>, u16, Option<&str>),
{
    let mut current_auth_token = auth_token.to_string();
    let mut status = upstream.status();
    if !status.is_success() {
        log::warn!(
            "gateway upstream non-success: status={}, account_id={}",
            status,
            account.id
        );
    }

    if status.as_u16() == 401 {
        match try_refresh_chatgpt_access_token(storage, upstream_base, account, token) {
            Ok(Some(refreshed_auth_token)) => {
                current_auth_token = refreshed_auth_token;
                if debug {
                    log::warn!(
                        "event=gateway_upstream_unauthorized_refresh_retry path={} account_id={}",
                        path,
                        account.id
                    );
                }
                match super::transport::send_upstream_request(
                    client,
                    method,
                    url,
                    request_deadline,
                    request_ctx,
                    incoming_headers,
                    body,
                    is_stream,
                    current_auth_token.as_str(),
                    account,
                    strip_session_affinity,
                ) {
                    Ok(resp) => {
                        upstream = resp;
                        status = upstream.status();
                    }
                    Err(err) => {
                        log::warn!(
                            "event=gateway_upstream_unauthorized_refresh_retry_error path={} status=502 account_id={} err={}",
                            path,
                            account.id,
                            err
                        );
                    }
                }
            }
            Ok(None) => {}
            Err(err) => {
                let refresh_token_invalid =
                    mark_account_unavailable_for_refresh_token_error(storage, &account.id, &err);
                log::warn!(
                    "event=gateway_upstream_unauthorized_refresh_failed path={} account_id={} err={}",
                    path,
                    account.id,
                    err
                );
                if refresh_token_invalid && has_more_candidates {
                    log_gateway_result(Some(url), 401, Some("refresh token invalid failover"));
                    return PostRetryFlowDecision::Failover;
                }
            }
        }
    }

    if let Some(alt_url) = url_alt {
        match retry_with_alternate_path(
            client,
            method,
            Some(alt_url),
            request_deadline,
            request_ctx,
            incoming_headers,
            body,
            is_stream,
            current_auth_token.as_str(),
            account,
            strip_session_affinity,
            status,
            debug,
            has_more_candidates,
            &mut log_gateway_result,
        ) {
            AltPathRetryResult::NotTriggered => {}
            AltPathRetryResult::Upstream(resp) => {
                upstream = resp;
                status = upstream.status();
            }
            AltPathRetryResult::Failover => {
                return PostRetryFlowDecision::Failover;
            }
            AltPathRetryResult::Terminal {
                status_code,
                message,
            } => {
                return PostRetryFlowDecision::Terminal {
                    status_code,
                    message,
                };
            }
        }
    }

    match retry_upstream_server_error_once(
        client,
        method,
        url,
        request_deadline,
        request_ctx,
        incoming_headers,
        body,
        is_stream,
        current_auth_token.as_str(),
        account,
        strip_session_affinity,
        debug,
        status,
    ) {
        Ok(Some(resp)) => {
            upstream = resp;
            status = upstream.status();
        }
        Ok(None) => {}
        Err(()) => {
            return PostRetryFlowDecision::Terminal {
                status_code: 504,
                message: "upstream total timeout exceeded".to_string(),
            };
        }
    }

    match retry_stateless_then_optional_alt(
        client,
        method,
        url,
        url_alt,
        request_deadline,
        request_ctx,
        incoming_headers,
        body,
        is_stream,
        current_auth_token.as_str(),
        account,
        strip_session_affinity,
        status,
        debug,
        disable_challenge_stateless_retry,
    ) {
        StatelessRetryResult::NotTriggered => {}
        StatelessRetryResult::Upstream(resp) => {
            upstream = resp;
            status = upstream.status();
        }
        StatelessRetryResult::Terminal {
            status_code,
            message,
        } => {
            return PostRetryFlowDecision::Terminal {
                status_code,
                message,
            };
        }
    }

    // 中文注释：主流程 fallback 只覆盖首跳响应，这里补齐“重试后仍 challenge/401/403/429”场景。
    match handle_openai_fallback_branch(
        client,
        storage,
        method,
        incoming_headers,
        body,
        is_stream,
        upstream_base,
        path,
        upstream_fallback_base,
        account,
        token,
        strip_session_affinity,
        debug,
        allow_openai_fallback,
        status,
        upstream.headers().get(reqwest::header::CONTENT_TYPE),
        has_more_candidates,
        &mut log_gateway_result,
    ) {
        FallbackBranchResult::NotTriggered => {}
        FallbackBranchResult::RespondUpstream(resp) => {
            return PostRetryFlowDecision::RespondUpstream(resp);
        }
        FallbackBranchResult::Failover => {
            return PostRetryFlowDecision::Failover;
        }
        FallbackBranchResult::Terminal {
            status_code,
            message,
        } => {
            return PostRetryFlowDecision::Terminal {
                status_code,
                message,
            };
        }
    }

    match decide_upstream_outcome(
        storage,
        &account.id,
        status,
        upstream.headers().get(reqwest::header::CONTENT_TYPE),
        url,
        has_more_candidates,
        &mut log_gateway_result,
    ) {
        UpstreamOutcomeDecision::Failover => PostRetryFlowDecision::Failover,
        UpstreamOutcomeDecision::RespondUpstream => {
            PostRetryFlowDecision::RespondUpstream(upstream)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::IncomingHeaderSnapshot;
    use codexmanager_core::storage::{now_ts, Account, Storage, Token};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;
    use tiny_http::{Response, Server, StatusCode};

    fn build_account(id: &str, now: i64) -> Account {
        Account {
            id: id.to_string(),
            label: id.to_string(),
            issuer: "https://auth.openai.com".to_string(),
            chatgpt_account_id: Some("chatgpt-account".to_string()),
            workspace_id: Some("workspace-account".to_string()),
            group_name: None,
            sort: 0,
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        }
    }

    fn build_token(account_id: &str, now: i64) -> Token {
        Token {
            account_id: account_id.to_string(),
            id_token: "id-token".to_string(),
            access_token: "access-token".to_string(),
            refresh_token: String::new(),
            api_key_access_token: Some("api-key-token".to_string()),
            last_refresh: now,
        }
    }

    #[test]
    fn retries_server_error_once_before_final_decision() {
        let storage = Storage::open_in_memory().expect("open storage");
        storage.init().expect("init storage");
        let now = now_ts();
        let account = build_account("acc-500-retry", now);
        let mut token = build_token(account.id.as_str(), now);
        let auth_token = token.access_token.clone();
        storage.insert_account(&account).expect("insert account");
        storage.insert_token(&token).expect("insert token");

        let server = Server::http("127.0.0.1:0").expect("start server");
        let addr = format!("http://{}", server.server_addr());
        let hit_count = Arc::new(AtomicUsize::new(0));
        let hit_count_thread = Arc::clone(&hit_count);
        let join = thread::spawn(move || {
            for (index, status) in [500u16, 200u16].into_iter().enumerate() {
                let mut request = server
                    .recv_timeout(Duration::from_secs(2))
                    .expect("receive upstream request")
                    .expect("request present");
                let mut body = Vec::new();
                let _ = request
                    .as_reader()
                    .read_to_end(&mut body)
                    .expect("read request body");
                hit_count_thread.fetch_add(1, Ordering::SeqCst);
                let response = Response::from_string(if index == 0 { "first" } else { "second" })
                    .with_status_code(StatusCode(status));
                request.respond(response).expect("respond");
            }
        });

        let client = reqwest::blocking::Client::new();
        let incoming_headers = IncomingHeaderSnapshot::default();
        let request_ctx = UpstreamRequestContext {
            request_path: "/v1/responses",
        };
        let body = Bytes::from_static(br#"{"model":"gpt-5.3-codex","input":"hello"}"#);
        let upstream = super::super::transport::send_upstream_request(
            &client,
            &reqwest::Method::POST,
            addr.as_str(),
            None,
            request_ctx,
            &incoming_headers,
            &body,
            false,
            auth_token.as_str(),
            &account,
            false,
        )
        .expect("send initial request");

        let decision = process_upstream_post_retry_flow(
            &client,
            &storage,
            &reqwest::Method::POST,
            addr.as_str(),
            "/v1/responses",
            addr.as_str(),
            None,
            None,
            request_ctx,
            &incoming_headers,
            &body,
            false,
            auth_token.as_str(),
            &account,
            &mut token,
            None,
            false,
            false,
            false,
            false,
            true,
            upstream,
            |_, _, _| {},
        );

        join.join().expect("join server");
        assert_eq!(hit_count.load(Ordering::SeqCst), 2);
        match decision {
            PostRetryFlowDecision::RespondUpstream(resp) => assert_eq!(resp.status(), 200),
            _ => panic!("unexpected decision"),
        }
    }
}
