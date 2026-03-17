use codexmanager_core::storage::Storage;
use reqwest::header::HeaderValue;

pub(in super::super) enum UpstreamOutcomeDecision {
    Failover,
    RespondUpstream,
}

pub(in super::super) fn decide_upstream_outcome<F>(
    storage: &Storage,
    account_id: &str,
    status: reqwest::StatusCode,
    upstream_content_type: Option<&HeaderValue>,
    url: &str,
    has_more_candidates: bool,
    mut log_gateway_result: F,
) -> UpstreamOutcomeDecision
where
    F: FnMut(Option<&str>, u16, Option<&str>),
{
    if status.is_success() {
        super::super::super::clear_account_cooldown(account_id);
        log_gateway_result(Some(url), status.as_u16(), None);
        return UpstreamOutcomeDecision::RespondUpstream;
    }
    if status.as_u16() == 404 && has_more_candidates {
        // 中文注释：模型/路径 404 在多账号场景下通常是“该账号不可用”，
        // 优先切换候选账号，最后一个候选再透传原始 404 给客户端。
        log_gateway_result(
            Some(url),
            status.as_u16(),
            Some("upstream not-found failover"),
        );
        return UpstreamOutcomeDecision::Failover;
    }
    if status.as_u16() == 429 {
        super::super::super::mark_account_cooldown(
            account_id,
            super::super::super::CooldownReason::RateLimited,
        );
        log_gateway_result(Some(url), status.as_u16(), Some("upstream rate-limited"));
        if has_more_candidates {
            return UpstreamOutcomeDecision::Failover;
        }
        return UpstreamOutcomeDecision::RespondUpstream;
    }

    let is_challenge =
        super::super::super::is_upstream_challenge_response(status.as_u16(), upstream_content_type);
    if is_challenge {
        super::super::super::mark_account_cooldown(
            account_id,
            super::super::super::CooldownReason::Challenge,
        );
        log_gateway_result(
            Some(url),
            status.as_u16(),
            Some("upstream challenge blocked"),
        );
        if has_more_candidates {
            return UpstreamOutcomeDecision::Failover;
        }
        return UpstreamOutcomeDecision::RespondUpstream;
    }

    if status.is_server_error() {
        super::super::super::mark_account_cooldown(
            account_id,
            super::super::super::CooldownReason::Upstream5xx,
        );
        log_gateway_result(Some(url), status.as_u16(), Some("upstream_http_error"));
        if has_more_candidates {
            return UpstreamOutcomeDecision::Failover;
        }
        return UpstreamOutcomeDecision::RespondUpstream;
    }
    let _ = storage;
    log_gateway_result(Some(url), status.as_u16(), Some("upstream_http_error"));
    UpstreamOutcomeDecision::RespondUpstream
}

#[cfg(test)]
#[path = "../tests/support/outcome_tests.rs"]
mod tests;
