use bytes::Bytes;
use codexmanager_core::storage::{Account, Storage, Token};
use reqwest::header::HeaderValue;

pub(super) enum FallbackBranchResult {
    NotTriggered,
    RespondUpstream(reqwest::blocking::Response),
    Failover,
    Terminal { status_code: u16, message: String },
}

fn should_failover_after_fallback_non_success(status: u16, has_more_candidates: bool) -> bool {
    if !has_more_candidates {
        return false;
    }
    matches!(status, 401 | 403 | 404 | 408 | 409 | 429)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_openai_fallback_branch<F>(
    client: &reqwest::blocking::Client,
    storage: &Storage,
    method: &reqwest::Method,
    request: &tiny_http::Request,
    incoming_headers: &super::super::super::IncomingHeaderSnapshot,
    body: &Bytes,
    is_stream: bool,
    upstream_base: &str,
    path: &str,
    fallback_base: Option<&str>,
    account: &Account,
    token: &mut Token,
    upstream_cookie: Option<&str>,
    strip_session_affinity: bool,
    debug: bool,
    allow_openai_fallback: bool,
    status: reqwest::StatusCode,
    upstream_content_type: Option<&HeaderValue>,
    has_more_candidates: bool,
    mut log_gateway_result: F,
) -> FallbackBranchResult
where
    F: FnMut(Option<&str>, u16, Option<&str>),
{
    if !allow_openai_fallback || fallback_base.is_none() {
        return FallbackBranchResult::NotTriggered;
    }

    let should_fallback =
        super::super::super::should_try_openai_fallback(upstream_base, path, upstream_content_type)
            || super::super::super::should_try_openai_fallback_by_status(
                upstream_base,
                path,
                status.as_u16(),
            );
    if !should_fallback {
        return FallbackBranchResult::NotTriggered;
    }

    let fallback_base = fallback_base.expect("fallback base already checked");
    if debug {
        log::warn!(
            "event=gateway_upstream_fallback path={} status={} account_id={} from={} to={}",
            path,
            status.as_u16(),
            account.id,
            upstream_base,
            fallback_base
        );
    }
    match super::super::super::try_openai_fallback(
        client,
        storage,
        method,
        path,
        request,
        incoming_headers,
        body,
        is_stream,
        fallback_base,
        account,
        token,
        upstream_cookie,
        strip_session_affinity,
        debug,
    ) {
        Ok(Some(resp)) => {
            if resp.status().is_success() {
                super::super::super::clear_account_cooldown(&account.id);
                log_gateway_result(Some(fallback_base), resp.status().as_u16(), None);
                return FallbackBranchResult::RespondUpstream(resp);
            }
            let fallback_status = resp.status().as_u16();
            super::super::super::mark_account_cooldown_for_status(&account.id, fallback_status);
            let fallback_error = format!(
                "upstream fallback non-success(primary_status={})",
                status.as_u16()
            );
            log_gateway_result(
                Some(fallback_base),
                fallback_status,
                Some(fallback_error.as_str()),
            );
            // 中文注释：仅对“可能账号相关/可恢复”的状态继续 failover；
            // 例如 5xx 这类上游服务端错误直接回传，避免单次请求在大量候选账号上长时间轮询。
            if should_failover_after_fallback_non_success(fallback_status, has_more_candidates) {
                FallbackBranchResult::Failover
            } else {
                FallbackBranchResult::RespondUpstream(resp)
            }
        }
        Ok(None) => {
            super::super::super::mark_account_cooldown(
                &account.id,
                super::super::super::CooldownReason::Network,
            );
            log_gateway_result(
                Some(fallback_base),
                502,
                Some("upstream fallback unavailable"),
            );
            if has_more_candidates {
                FallbackBranchResult::Failover
            } else {
                FallbackBranchResult::Terminal {
                    status_code: 502,
                    message: "upstream blocked by Cloudflare; set CODEXMANAGER_UPSTREAM_COOKIE"
                        .to_string(),
                }
            }
        }
        Err(err) => {
            super::super::super::mark_account_cooldown(
                &account.id,
                super::super::super::CooldownReason::Network,
            );
            log_gateway_result(Some(fallback_base), 502, Some(err.as_str()));
            if has_more_candidates {
                FallbackBranchResult::Failover
            } else {
                FallbackBranchResult::Terminal {
                    status_code: 502,
                    message: format!("upstream fallback error: {err}"),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/attempt_flow/fallback_branch_tests.rs"]
mod tests;
