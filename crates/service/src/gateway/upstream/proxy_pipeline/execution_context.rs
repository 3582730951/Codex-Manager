use super::super::support::candidates;
use codexmanager_core::storage::Storage;

pub(in super::super) struct GatewayUpstreamExecutionContext<'a> {
    trace_id: &'a str,
    storage: &'a Storage,
    key_id: &'a str,
    original_path: &'a str,
    path: &'a str,
    request_method: &'a str,
    response_adapter: super::super::super::ResponseAdapter,
    protocol_type: &'a str,
    model_for_log: Option<&'a str>,
    reasoning_for_log: Option<&'a str>,
    candidate_count: usize,
    account_max_inflight: usize,
}

impl<'a> GatewayUpstreamExecutionContext<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(in super::super) fn new(
        trace_id: &'a str,
        storage: &'a Storage,
        key_id: &'a str,
        original_path: &'a str,
        path: &'a str,
        request_method: &'a str,
        response_adapter: super::super::super::ResponseAdapter,
        protocol_type: &'a str,
        model_for_log: Option<&'a str>,
        reasoning_for_log: Option<&'a str>,
        candidate_count: usize,
        account_max_inflight: usize,
    ) -> Self {
        Self {
            trace_id,
            storage,
            key_id,
            original_path,
            path,
            request_method,
            response_adapter,
            protocol_type,
            model_for_log,
            reasoning_for_log,
            candidate_count,
            account_max_inflight,
        }
    }

    pub(super) fn has_more_candidates(&self, idx: usize) -> bool {
        idx + 1 < self.candidate_count
    }

    pub(super) fn should_skip_candidate(
        &self,
        account_id: &str,
        idx: usize,
    ) -> Option<candidates::CandidateSkipReason> {
        candidates::candidate_skip_reason_for_proxy(
            account_id,
            idx,
            self.candidate_count,
            self.account_max_inflight,
        )
    }

    pub(super) fn log_candidate_start(
        &self,
        account_id: &str,
        idx: usize,
        strip_session_affinity: bool,
    ) {
        super::super::super::trace_log::log_candidate_start(
            self.trace_id,
            idx,
            self.candidate_count,
            account_id,
            strip_session_affinity,
        );
    }

    pub(super) fn log_candidate_skip(
        &self,
        account_id: &str,
        idx: usize,
        reason: candidates::CandidateSkipReason,
    ) {
        let reason_text = match reason {
            candidates::CandidateSkipReason::Cooldown => "cooldown",
            candidates::CandidateSkipReason::Inflight => "inflight",
        };
        super::super::super::trace_log::log_candidate_skip(
            self.trace_id,
            idx,
            self.candidate_count,
            account_id,
            reason_text,
        );
    }

    pub(super) fn log_attempt_result(
        &self,
        account_id: &str,
        upstream_url: Option<&str>,
        status_code: u16,
        error: Option<&str>,
    ) {
        super::super::super::trace_log::log_attempt_result(
            self.trace_id,
            account_id,
            upstream_url,
            status_code,
            error,
        );
    }

    pub(in super::super) fn log_final_result(
        &self,
        final_account_id: Option<&str>,
        upstream_url: Option<&str>,
        status_code: u16,
        usage: super::super::super::request_log::RequestLogUsage,
        error: Option<&str>,
        elapsed_ms: u128,
    ) {
        super::super::super::write_request_log(
            self.storage,
            super::super::super::request_log::RequestLogTraceContext {
                trace_id: Some(self.trace_id),
                original_path: Some(self.original_path),
                adapted_path: Some(self.path),
                response_adapter: Some(self.response_adapter),
            },
            Some(self.key_id),
            final_account_id,
            self.path,
            self.request_method,
            self.model_for_log,
            self.reasoning_for_log,
            upstream_url,
            Some(status_code),
            usage,
            error,
            Some(elapsed_ms),
        );
        super::super::super::trace_log::log_request_final(
            self.trace_id,
            status_code,
            final_account_id,
            upstream_url,
            error,
            elapsed_ms,
        );
        super::super::super::record_gateway_request_outcome(
            self.path,
            status_code,
            Some(self.protocol_type),
        );
    }
}
