use bytes::Bytes;
use codexmanager_core::storage::{Account, Storage, Token};
use std::time::Instant;
use tiny_http::Request;

use super::super::attempt_flow::candidate_flow::{
    process_candidate_upstream_flow, CandidateUpstreamDecision,
};
use super::execution_context::GatewayUpstreamExecutionContext;
use super::request_setup::UpstreamRequestSetup;

#[derive(Default)]
pub(super) struct CandidateAttemptTrace {
    pub(super) last_attempt_url: Option<String>,
    pub(super) last_attempt_error: Option<String>,
}

pub(super) struct CandidateAttemptParams<'a> {
    pub(super) storage: &'a Storage,
    pub(super) method: &'a reqwest::Method,
    pub(super) request: &'a Request,
    pub(super) incoming_headers: &'a super::super::super::IncomingHeaderSnapshot,
    pub(super) body: &'a Bytes,
    pub(super) upstream_is_stream: bool,
    pub(super) path: &'a str,
    pub(super) request_deadline: Option<Instant>,
    pub(super) account: &'a Account,
    pub(super) token: &'a mut Token,
    pub(super) strip_session_affinity: bool,
    pub(super) debug: bool,
    pub(super) allow_openai_fallback: bool,
    pub(super) disable_challenge_stateless_retry: bool,
    pub(super) has_more_candidates: bool,
    pub(super) context: &'a GatewayUpstreamExecutionContext<'a>,
    pub(super) setup: &'a UpstreamRequestSetup,
    pub(super) trace: &'a mut CandidateAttemptTrace,
}

fn should_record_route_quality_penalty(error: Option<&str>) -> bool {
    let normalized = error
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    !matches!(
        normalized.as_deref(),
        Some("likely_model_ineligible_failover")
            | Some("likely_quota_rejected_failover")
            | Some("upstream not-found failover")
    )
}

pub(super) fn run_candidate_attempt(
    params: CandidateAttemptParams<'_>,
) -> CandidateUpstreamDecision {
    let CandidateAttemptParams {
        storage,
        method,
        request,
        incoming_headers,
        body,
        upstream_is_stream,
        path,
        request_deadline,
        account,
        token,
        strip_session_affinity,
        debug,
        allow_openai_fallback,
        disable_challenge_stateless_retry,
        has_more_candidates,
        context,
        setup,
        trace,
    } = params;

    let gate_guard = super::request_gate::acquire_request_gate(
        context.trace_id(),
        context.key_id(),
        path,
        context.model_for_log(),
        request_deadline,
    );
    if gate_guard.is_none() {
        if super::super::support::deadline::is_expired(request_deadline) {
            return CandidateUpstreamDecision::Terminal {
                status_code: 504,
                message: "upstream total timeout exceeded".to_string(),
            };
        }
        return CandidateUpstreamDecision::Terminal {
            status_code: 503,
            message: "request gate rejected".to_string(),
        };
    }

    let decision = process_candidate_upstream_flow(
        storage,
        method,
        request,
        incoming_headers,
        body,
        upstream_is_stream,
        context.model_for_log(),
        setup.upstream_base.as_str(),
        path,
        setup.url.as_str(),
        setup.url_alt.as_deref(),
        request_deadline,
        setup.upstream_fallback_base.as_deref(),
        account,
        token,
        setup.upstream_cookie.as_deref(),
        strip_session_affinity,
        debug,
        allow_openai_fallback,
        disable_challenge_stateless_retry,
        has_more_candidates,
        |upstream_url: Option<&str>, status_code, error: Option<&str>| {
            trace.last_attempt_url = upstream_url.map(str::to_string);
            trace.last_attempt_error = error.map(str::to_string);
            if should_record_route_quality_penalty(error) {
                super::super::super::record_route_quality(&account.id, status_code);
            }
            context.log_attempt_result(&account.id, upstream_url, status_code, error);
        },
    );
    drop(gate_guard);
    decision
}
