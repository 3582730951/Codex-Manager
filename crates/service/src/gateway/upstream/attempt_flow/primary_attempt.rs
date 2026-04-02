use codexmanager_core::storage::Account;
use std::time::{Duration, Instant};

use super::super::support::deadline;
use super::transport::UpstreamRequestContext;

pub(super) enum PrimaryAttemptResult {
    Upstream(reqwest::blocking::Response),
    Failover,
    Terminal { status_code: u16, message: String },
}

const COMPACT_FAILOVER_RESERVE_MS: u64 = 30_000;

fn is_compact_request_path(request_path: &str) -> bool {
    request_path == "/v1/responses/compact" || request_path.starts_with("/v1/responses/compact?")
}

fn compact_retry_target(request_path: &str, url: &str) -> Option<(String, String)> {
    if !is_compact_request_path(request_path) {
        return None;
    }
    let fallback_path = request_path.replacen("/v1/responses/compact", "/v1/responses", 1);
    let fallback_url = url.replacen("/responses/compact", "/responses", 1);
    (fallback_url != url).then_some((fallback_path, fallback_url))
}

fn compact_budget_too_low_for_same_account_retry(
    request_deadline: Option<Instant>,
    has_more_candidates: bool,
) -> bool {
    has_more_candidates
        && deadline::remaining(request_deadline).is_some_and(|remaining| {
            remaining <= Duration::from_millis(COMPACT_FAILOVER_RESERVE_MS)
        })
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
    let compact_retry = compact_retry_target(request_ctx.request_path, url);
    let mut primary_path_override = None;
    let mut primary_url_override = None;
    let mut bypassed_compact = false;
    if let Some((fallback_path, fallback_url)) = compact_retry.as_ref() {
        if compact_budget_too_low_for_same_account_retry(request_deadline, has_more_candidates) {
            log::warn!(
                "event=gateway_compact_low_budget_bypass path={} compact_url={} fallback_url={} remaining_ms={} reserve_ms={}",
                request_ctx.request_path,
                url,
                fallback_url,
                deadline::remaining(request_deadline)
                    .map(|value| value.as_millis())
                    .unwrap_or(0),
                COMPACT_FAILOVER_RESERVE_MS,
            );
            primary_path_override = Some(fallback_path.clone());
            primary_url_override = Some(fallback_url.clone());
            bypassed_compact = true;
        }
    }
    let primary_request_path = primary_path_override
        .as_deref()
        .unwrap_or(request_ctx.request_path);
    let primary_url = primary_url_override.as_deref().unwrap_or(url);
    let primary_request_ctx = UpstreamRequestContext {
        request_path: primary_request_path,
    };
    if deadline::is_expired(request_deadline) {
        log_gateway_result(
            Some(primary_url),
            504,
            Some("upstream total timeout exceeded"),
        );
        return PrimaryAttemptResult::Terminal {
            status_code: 504,
            message: "upstream total timeout exceeded".to_string(),
        };
    }
    match super::transport::send_upstream_request(
        client,
        method,
        primary_url,
        request_deadline,
        primary_request_ctx,
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
            if !bypassed_compact
                && compact_budget_too_low_for_same_account_retry(
                    request_deadline,
                    has_more_candidates,
                )
            {
                log::warn!(
                    "event=gateway_compact_low_budget_failover path={} compact_url={} remaining_ms={} reserve_ms={} err={}",
                    request_ctx.request_path,
                    primary_url,
                    deadline::remaining(request_deadline)
                        .map(|value| value.as_millis())
                        .unwrap_or(0),
                    COMPACT_FAILOVER_RESERVE_MS,
                    err_msg,
                );
            } else if let Some((fallback_path, fallback_url)) = compact_retry {
                log::warn!(
                    "event=gateway_compact_transport_fallback path={} compact_url={} fallback_url={} account_id={} err={}",
                    request_ctx.request_path,
                    primary_url,
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
                        err_msg = format!("{err_msg}; compact_fallback_retry: {}", fallback_err);
                    }
                }
            }
            super::super::super::mark_account_cooldown(
                &account.id,
                super::super::super::CooldownReason::Network,
            );
            log_gateway_result(Some(primary_url), 502, Some(err_msg.as_str()));
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
    use super::{
        compact_budget_too_low_for_same_account_retry, compact_retry_target,
        is_compact_request_path,
    };
    use std::time::{Duration, Instant};

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

    #[test]
    fn compact_path_detection_matches_compact_variants() {
        assert!(is_compact_request_path("/v1/responses/compact"));
        assert!(is_compact_request_path(
            "/v1/responses/compact?stream=false"
        ));
        assert!(!is_compact_request_path("/v1/responses"));
    }

    #[test]
    fn compact_budget_reserve_only_triggers_with_low_remaining_budget_and_other_candidates() {
        let low_budget = Some(Instant::now() + Duration::from_secs(5));
        assert!(compact_budget_too_low_for_same_account_retry(
            low_budget, true
        ));
        assert!(!compact_budget_too_low_for_same_account_retry(
            low_budget, false
        ));

        let healthy_budget = Some(Instant::now() + Duration::from_secs(90));
        assert!(!compact_budget_too_low_for_same_account_retry(
            healthy_budget,
            true
        ));
    }
}
