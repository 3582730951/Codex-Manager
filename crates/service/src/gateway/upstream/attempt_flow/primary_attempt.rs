use codexmanager_core::storage::Account;
use std::time::Instant;

use super::super::support::deadline;
use super::transport::UpstreamRequestContext;

pub(super) enum PrimaryAttemptResult {
    Upstream(reqwest::blocking::Response),
    Failover,
    Terminal { status_code: u16, message: String },
}

fn compact_retry_target(request_path: &str, url: &str) -> Option<(String, String)> {
    if !(request_path == "/v1/responses/compact"
        || request_path.starts_with("/v1/responses/compact?"))
    {
        return None;
    }
    let fallback_path = request_path.replacen("/v1/responses/compact", "/v1/responses", 1);
    let fallback_url = url.replacen("/responses/compact", "/responses", 1);
    (fallback_url != url).then_some((fallback_path, fallback_url))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_primary_upstream_attempt<F>(
    client: &reqwest::blocking::Client,
    method: &reqwest::Method,
    url: &str,
    request_deadline: Option<Instant>,
    request_ctx: UpstreamRequestContext<'_>,
    incoming_headers: &super::super::super::IncomingHeaderSnapshot,
    body: &crate::gateway::RequestPayload,
    is_stream: bool,
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
        request_ctx,
        incoming_headers,
        body,
        is_stream,
        auth_token,
        account,
        strip_session_affinity,
    ) {
        Ok(resp) => PrimaryAttemptResult::Upstream(resp),
        Err(err) => {
            let mut err_msg = err.to_string();
            if let Some((fallback_path, fallback_url)) =
                compact_retry_target(request_ctx.request_path, url)
            {
                log::warn!(
                    "event=gateway_compact_transport_fallback path={} compact_url={} fallback_url={} account_id={} err={}",
                    request_ctx.request_path,
                    url,
                    fallback_url,
                    account.id,
                    err_msg,
                );
                let fallback_ctx = UpstreamRequestContext {
                    request_path: fallback_path.as_str(),
                };
                match super::transport::send_upstream_request(
                    client,
                    method,
                    fallback_url.as_str(),
                    request_deadline,
                    fallback_ctx,
                    incoming_headers,
                    body,
                    is_stream,
                    auth_token,
                    account,
                    strip_session_affinity,
                ) {
                    Ok(resp) => {
                        return PrimaryAttemptResult::Upstream(resp);
                    }
                    Err(fallback_err) => {
                        err_msg =
                            format!("{err_msg}; compact_fallback_retry: {}", fallback_err);
                    }
                }
            }
            super::super::super::mark_account_cooldown(
                &account.id,
                super::super::super::CooldownReason::Network,
            );
            log_gateway_result(Some(url), 502, Some(err_msg.as_str()));
            // 中文注释：主链路首次请求失败不代表所有候选都失败，
            // 先 failover 才能避免单账号抖动放大成全局不可用。
            if has_more_candidates {
                PrimaryAttemptResult::Failover
            } else {
                PrimaryAttemptResult::Terminal {
                    status_code: 502,
                    message: format!("upstream error: {err}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::compact_retry_target;

    #[test]
    fn compact_retry_target_rewrites_standard_responses_path_and_url() {
        let (path, url) = compact_retry_target(
            "/v1/responses/compact?stream=false",
            "https://chatgpt.com/backend-api/codex/responses/compact?stream=false",
        )
        .expect("compact retry target");

        assert_eq!(path, "/v1/responses?stream=false");
        assert_eq!(
            url,
            "https://chatgpt.com/backend-api/codex/responses?stream=false"
        );
    }

    #[test]
    fn compact_retry_target_ignores_non_compact_paths() {
        assert!(compact_retry_target(
            "/v1/responses",
            "https://chatgpt.com/backend-api/codex/responses"
        )
        .is_none());
    }
}
