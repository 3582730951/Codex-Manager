use bytes::Bytes;
use codexmanager_core::storage::Account;
use std::time::Instant;

use super::super::support::deadline;

pub(super) enum PrimaryAttemptResult {
    Upstream(reqwest::blocking::Response),
    Failover,
    Terminal { status_code: u16, message: String },
}

fn classify_preheader_error(err: &reqwest::Error) -> (&'static str, u16, bool) {
    if err.is_timeout() {
        return ("upstream_preheader_timeout", 504, true);
    }
    if err.is_connect() {
        return ("upstream_connect_failure", 502, true);
    }
    ("upstream_connect_failure", 502, false)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_primary_upstream_attempt<F>(
    client: &reqwest::blocking::Client,
    method: &reqwest::Method,
    url: &str,
    request_deadline: Option<Instant>,
    request: &tiny_http::Request,
    incoming_headers: &super::super::super::IncomingHeaderSnapshot,
    body: &Bytes,
    is_stream: bool,
    upstream_cookie: Option<&str>,
    auth_token: &str,
    account: &Account,
    strip_session_affinity: bool,
    has_more_candidates: bool,
    mut log_gateway_result: F,
) -> PrimaryAttemptResult
where
    F: FnMut(Option<&str>, u16, Option<&str>),
{
    if deadline::is_expired(request_deadline) {
        log_gateway_result(Some(url), 504, Some("upstream total timeout exceeded"));
        return PrimaryAttemptResult::Terminal {
            status_code: 504,
            message: "upstream total timeout exceeded".to_string(),
        };
    }
    match super::transport::send_upstream_request(
        client,
        method,
        url,
        request_deadline,
        request,
        incoming_headers,
        body,
        is_stream,
        upstream_cookie,
        auth_token,
        account,
        strip_session_affinity,
    ) {
        Ok(resp) => PrimaryAttemptResult::Upstream(resp),
        Err(err) => {
            let (error_code, status_code, should_cooldown) = classify_preheader_error(&err);
            if should_cooldown {
                super::super::super::mark_account_cooldown(
                    &account.id,
                    super::super::super::CooldownReason::Network,
                );
                let _ = super::super::super::clear_manual_preferred_account_if(&account.id);
            }
            log_gateway_result(Some(url), status_code, Some(error_code));
            // 中文注释：主链路首次请求失败不代表所有候选都失败，
            // 先 failover 才能避免单账号抖动放大成全局不可用。
            if has_more_candidates {
                PrimaryAttemptResult::Failover
            } else {
                PrimaryAttemptResult::Terminal {
                    status_code,
                    message: error_code.to_string(),
                }
            }
        }
    }
}
