use codexmanager_core::storage::Storage;
use reqwest::header::HeaderValue;

pub(in super::super) enum UpstreamOutcomeDecision {
    Failover,
    RespondUpstream,
}

fn should_failover_after_account_non_success(status: u16, has_more_candidates: bool) -> bool {
    if !has_more_candidates {
        return false;
    }
    matches!(status, 401 | 402 | 403 | 404 | 408 | 409 | 429)
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
    let is_challenge =
        super::super::super::is_upstream_challenge_response(status.as_u16(), upstream_content_type);
    if is_challenge {
        super::super::super::mark_account_cooldown(
            account_id,
            super::super::super::CooldownReason::Challenge,
        );
        let _ = super::super::super::clear_manual_preferred_account_if(account_id);
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

    if status.is_success() {
        super::super::super::clear_account_cooldown(account_id);
        log_gateway_result(Some(url), status.as_u16(), None);
        return UpstreamOutcomeDecision::RespondUpstream;
    }
    if matches!(status.as_u16(), 401 | 402 | 403 | 408 | 409 | 429) {
        super::super::super::mark_account_cooldown_for_status(account_id, status.as_u16());
        let _ = super::super::super::clear_manual_preferred_account_if(account_id);
    }
    if should_failover_after_account_non_success(status.as_u16(), has_more_candidates) {
        let error = match status.as_u16() {
            401 => "upstream unauthorized failover",
            402 => "upstream quota exhausted failover",
            403 => "upstream forbidden failover",
            404 => "upstream not-found failover",
            408 => "upstream request-timeout failover",
            409 => "upstream conflict failover",
            429 => "upstream rate-limited",
            _ => "upstream account-state failover",
        };
        log_gateway_result(Some(url), status.as_u16(), Some(error));
        return UpstreamOutcomeDecision::Failover;
    }

    if status.is_server_error() {
        super::super::super::mark_account_cooldown(
            account_id,
            super::super::super::CooldownReason::Upstream5xx,
        );
        let _ = super::super::super::clear_manual_preferred_account_if(account_id);
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
