use bytes::Bytes;
use codexmanager_core::storage::{Account, Storage, Token};

use super::super::support::outcome::{decide_upstream_outcome, UpstreamOutcomeDecision};

pub(super) enum OpenAiAttemptResult {
    Upstream(reqwest::blocking::Response),
    Failover,
    Terminal { status_code: u16, message: String },
}

pub(super) fn handle_openai_base_attempt<F>(
    client: &reqwest::blocking::Client,
    storage: &Storage,
    method: &reqwest::Method,
    path: &str,
    request: &tiny_http::Request,
    incoming_headers: &super::super::super::IncomingHeaderSnapshot,
    body: &Bytes,
    is_stream: bool,
    base: &str,
    account: &Account,
    token: &mut Token,
    upstream_cookie: Option<&str>,
    strip_session_affinity: bool,
    debug: bool,
    has_more_candidates: bool,
    mut log_gateway_result: F,
) -> OpenAiAttemptResult
where
    F: FnMut(Option<&str>, u16, Option<&str>),
{
    match super::super::super::try_openai_fallback(
        client,
        storage,
        method,
        path,
        request,
        incoming_headers,
        body,
        is_stream,
        base,
        account,
        token,
        upstream_cookie,
        strip_session_affinity,
        debug,
    ) {
        Ok(Some(resp)) => {
            match decide_upstream_outcome(
                storage,
                &account.id,
                resp.status(),
                resp.headers().get(reqwest::header::CONTENT_TYPE),
                base,
                has_more_candidates,
                &mut log_gateway_result,
            ) {
                UpstreamOutcomeDecision::Failover => OpenAiAttemptResult::Failover,
                UpstreamOutcomeDecision::RespondUpstream => OpenAiAttemptResult::Upstream(resp),
            }
        }
        Ok(None) => {
            log_gateway_result(Some(base), 502, Some("upstream_connect_failure"));
            let _ = super::super::super::clear_manual_preferred_account_if(&account.id);
            // 中文注释：OpenAI 上游不可用时如果还有候选账号就继续 failover，
            // 不这样做会把单账号瞬时抖动放大成整次请求失败。
            if has_more_candidates {
                OpenAiAttemptResult::Failover
            } else {
                OpenAiAttemptResult::Terminal {
                    status_code: 502,
                    message: "upstream_connect_failure".to_string(),
                }
            }
        }
        Err(err) => {
            log_gateway_result(Some(base), 502, Some(err.as_str()));
            let _ = super::super::super::clear_manual_preferred_account_if(&account.id);
            // 中文注释：异常分支同样优先切换候选账号，
            // 只有最后一个候选才直接向客户端返回错误，避免过早失败。
            if has_more_candidates {
                OpenAiAttemptResult::Failover
            } else {
                OpenAiAttemptResult::Terminal {
                    status_code: 502,
                    message: format!("openai upstream error: {err}"),
                }
            }
        }
    }
}
