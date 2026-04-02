use codexmanager_core::storage::{Account, Storage, Token};
use std::time::Instant;

use super::super::attempt_flow::candidate_flow::{
    process_candidate_upstream_flow, CandidateUpstreamDecision,
};
use super::super::attempt_flow::transport::UpstreamRequestContext;
use super::execution_context::GatewayUpstreamExecutionContext;
use super::request_setup::UpstreamRequestSetup;

#[derive(Default)]
pub(in super::super) struct CandidateAttemptTrace {
    pub(in super::super) last_attempt_url: Option<String>,
    pub(in super::super) last_attempt_error: Option<String>,
    pub(in super::super) last_status_code: Option<u16>,
}

pub(in super::super) struct CandidateAttemptParams<'a> {
    pub(in super::super) storage: &'a Storage,
    pub(in super::super) method: &'a reqwest::Method,
    pub(in super::super) request_ctx: UpstreamRequestContext<'a>,
    pub(in super::super) incoming_headers: &'a super::super::super::IncomingHeaderSnapshot,
    pub(in super::super) body: &'a crate::gateway::RequestPayload,
    pub(in super::super) client_is_stream: bool,
    pub(in super::super) upstream_is_stream: bool,
    pub(in super::super) path: &'a str,
    pub(in super::super) request_deadline: Option<Instant>,
    pub(in super::super) account: &'a Account,
    pub(in super::super) token: &'a mut Token,
    pub(in super::super) strip_session_affinity: bool,
    pub(in super::super) debug: bool,
    pub(in super::super) allow_openai_fallback: bool,
    pub(in super::super) disable_challenge_stateless_retry: bool,
    pub(in super::super) has_more_candidates: bool,
    pub(in super::super) context: &'a GatewayUpstreamExecutionContext<'a>,
    pub(in super::super) setup: &'a UpstreamRequestSetup,
    pub(in super::super) trace: &'a mut CandidateAttemptTrace,
}

pub(in super::super) fn run_candidate_attempt(
    params: CandidateAttemptParams<'_>,
) -> CandidateUpstreamDecision {
    let CandidateAttemptParams {
        storage,
        method,
        request_ctx,
        incoming_headers,
        body,
        client_is_stream,
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

    process_candidate_upstream_flow(
        storage,
        method,
        request_ctx,
        incoming_headers,
        body,
        client_is_stream,
        upstream_is_stream,
        setup.upstream_base.as_str(),
        path,
        setup.url.as_str(),
        setup.url_alt.as_deref(),
        request_deadline,
        setup.upstream_fallback_base.as_deref(),
        account,
        token,
        strip_session_affinity,
        debug,
        allow_openai_fallback,
        disable_challenge_stateless_retry,
        has_more_candidates,
        |upstream_url: Option<&str>, status_code, error: Option<&str>| {
            trace.last_attempt_url = upstream_url.map(str::to_string);
            trace.last_attempt_error = error.map(str::to_string);
            trace.last_status_code = Some(status_code);
            super::super::super::record_route_quality(&account.id, status_code);
            context.log_attempt_result(&account.id, upstream_url, status_code, error);
        },
    )
}
