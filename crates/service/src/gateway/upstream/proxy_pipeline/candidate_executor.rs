use codexmanager_core::storage::{Account, Storage, Token};
use std::time::Instant;
use tiny_http::Request;

use super::super::attempt_flow::candidate_flow::CandidateUpstreamDecision;
use super::super::attempt_flow::transport::UpstreamRequestContext;
use super::super::support::candidates::free_account_model_override;
use super::super::support::deadline;
use super::candidate_attempt::{
    run_candidate_attempt, CandidateAttemptParams, CandidateAttemptTrace,
};
use super::candidate_state::CandidateExecutionState;
use super::execution_context::GatewayUpstreamExecutionContext;
use super::request_setup::UpstreamRequestSetup;
use super::response_finalize::{
    finalize_terminal_candidate, finalize_upstream_response, respond_terminal,
    respond_total_timeout,
};

fn extract_prompt_cache_key_for_trace(body: &crate::gateway::RequestPayload) -> Option<String> {
    if body.is_empty() || body.len() > 64 * 1024 {
        return None;
    }
    let value = body.read_json_value().ok()?;
    value
        .get("prompt_cache_key")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(in super::super) enum CandidateExecutionResult {
    Handled,
    Exhausted {
        request: Request,
        attempted_account_ids: Vec<String>,
        skipped_cooldown: usize,
        skipped_inflight: usize,
        last_attempt_url: Option<String>,
        last_attempt_error: Option<String>,
    },
}

pub(in super::super) struct CandidateExecutorParams<'a> {
    pub(in super::super) storage: &'a Storage,
    pub(in super::super) method: &'a reqwest::Method,
    pub(in super::super) incoming_headers: &'a super::super::super::IncomingHeaderSnapshot,
    pub(in super::super) body: &'a crate::gateway::RequestPayload,
    pub(in super::super) path: &'a str,
    pub(in super::super) request_shape: Option<&'a str>,
    pub(in super::super) trace_id: &'a str,
    pub(in super::super) model_for_log: Option<&'a str>,
    pub(in super::super) response_adapter: super::super::super::ResponseAdapter,
    pub(in super::super) tool_name_restore_map: &'a super::super::super::ToolNameRestoreMap,
    pub(in super::super) context: &'a GatewayUpstreamExecutionContext<'a>,
    pub(in super::super) setup: &'a UpstreamRequestSetup,
    pub(in super::super) request_deadline: Option<Instant>,
    pub(in super::super) started_at: Instant,
    pub(in super::super) client_is_stream: bool,
    pub(in super::super) upstream_is_stream: bool,
    pub(in super::super) debug: bool,
    pub(in super::super) allow_openai_fallback: bool,
    pub(in super::super) disable_challenge_stateless_retry: bool,
}

pub(in super::super) fn execute_candidate_sequence(
    request: Request,
    candidates: Vec<(Account, Token)>,
    params: CandidateExecutorParams<'_>,
) -> Result<CandidateExecutionResult, String> {
    let CandidateExecutorParams {
        storage,
        method,
        incoming_headers,
        body,
        path,
        request_shape,
        trace_id,
        model_for_log,
        response_adapter,
        tool_name_restore_map,
        context,
        setup,
        request_deadline,
        started_at,
        client_is_stream,
        upstream_is_stream,
        debug,
        allow_openai_fallback,
        disable_challenge_stateless_retry,
    } = params;
    let mut request = Some(request);
    let mut state = CandidateExecutionState::default();
    let mut candidates = candidates;
    let mut attempted_account_ids = Vec::new();
    let mut skipped_cooldown = 0usize;
    let mut skipped_inflight = 0usize;
    let mut last_attempt_url = None;
    let mut last_attempt_error = None;

    loop {
        let mut skipped_this_round = 0usize;
        for idx in 0..candidates.len() {
            if deadline::is_expired(request_deadline) {
                let request = request
                    .take()
                    .expect("request should be available before timeout response");
                respond_total_timeout(
                    request,
                    context,
                    trace_id,
                    started_at,
                    model_for_log,
                    Some(attempted_account_ids.as_slice()),
                )?;
                return Ok(CandidateExecutionResult::Handled);
            }

            let (account, token) = candidates
                .get_mut(idx)
                .expect("candidate should exist for scheduler loop");
            let legacy_attempt_thread =
                super::super::super::conversation_binding::resolve_attempt_thread(
                    setup.legacy_conversation_routing(),
                    account,
                );
            let mut attempt_affinity_resolution = setup.persistent_affinity_resolution().cloned();
            let (attempt_thread_anchor, reset_session_affinity, persistent_body_override) =
                if let Some(resolution) = attempt_affinity_resolution.as_mut() {
                    let (_, thread_anchor, reset_session_affinity) =
                        super::super::super::affinity::resolve_attempt_thread_assignment(
                            resolution,
                            context.platform_key_hash(),
                            account.id.as_str(),
                        );
                    let persistent_body_override =
                        match super::super::super::affinity::build_attempt_replay_body(
                            storage,
                            resolution,
                            path,
                            body,
                            context.platform_key_hash(),
                            account.id.as_str(),
                        ) {
                            Ok(body_override) => body_override,
                            Err(err) if err == "affinity_migration_context_unavailable" => {
                                log::warn!(
                                    "event=gateway_affinity_replay_session_reset_late trace_id={} affinity_key={} previous_account={} chosen_account={} fallback_account={} reason={}",
                                    trace_id,
                                    resolution.affinity_key,
                                    resolution
                                        .thread
                                        .as_ref()
                                        .map(|item| item.account_id.as_str())
                                        .unwrap_or("-"),
                                    resolution.chosen_account_id,
                                    account.id,
                                    err,
                                );
                                *resolution =
                                    super::super::super::affinity::resolution_with_session_reset_recovery(
                                        resolution,
                                        account.id.as_str(),
                                    );
                                None
                            }
                            Err(err) => {
                                let request = request.take().expect(
                                    "request should be available before replay failure response",
                                );
                                context.log_final_result_with_model(
                                    Some(account.id.as_str()),
                                    None,
                                    model_for_log,
                                    409,
                                    super::super::super::request_log::RequestLogUsage::default(),
                                    Some(err.as_str()),
                                    started_at.elapsed().as_millis(),
                                    Some(attempted_account_ids.as_slice()),
                                );
                                respond_terminal(request, 409, err, Some(trace_id))?;
                                return Ok(CandidateExecutionResult::Handled);
                            }
                        };
                    (
                        Some(thread_anchor),
                        reset_session_affinity,
                        persistent_body_override,
                    )
                } else {
                    legacy_attempt_thread
                        .as_ref()
                        .map(|thread| {
                            (
                                Some(thread.thread_anchor.clone()),
                                thread.reset_session_affinity,
                                None,
                            )
                        })
                        .unwrap_or((None, false, None))
                };
            let strip_session_affinity = state.strip_session_affinity(
                account,
                idx,
                setup.anthropic_has_prompt_cache_key || attempt_thread_anchor.is_some(),
            );
            let attempt_headers = if let Some(thread_anchor) = attempt_thread_anchor.as_deref() {
                incoming_headers
                    .with_thread_affinity_override(Some(thread_anchor), reset_session_affinity)
            } else {
                incoming_headers.clone()
            };
            let attempt_model_override = free_account_model_override(storage, account, token);
            let attempt_model_for_log = attempt_model_override.as_deref().or(model_for_log);
            let body_for_attempt = state.body_for_attempt(
                path,
                body,
                strip_session_affinity,
                setup,
                persistent_body_override.as_ref(),
                attempt_model_override.as_deref(),
                attempt_thread_anchor.as_deref(),
            );
            context.log_candidate_start(&account.id, idx, strip_session_affinity);
            if let Some(skip_reason) = context.should_skip_candidate(&account.id, idx) {
                context.log_candidate_skip(&account.id, idx, skip_reason);
                skipped_this_round += 1;
                match skip_reason {
                    super::super::support::candidates::CandidateSkipReason::Cooldown => {
                        skipped_cooldown += 1;
                    }
                    super::super::support::candidates::CandidateSkipReason::Inflight => {
                        skipped_inflight += 1;
                    }
                }
                continue;
            }
            super::super::super::record_scheduler_assignment(&account.id);
            attempted_account_ids.push(account.id.clone());

            let request_ref = request
                .as_ref()
                .ok_or_else(|| "request already consumed".to_string())?;
            let request_ctx = UpstreamRequestContext::from_request(request_ref);
            let incoming_session_id = attempt_headers.session_id();
            let incoming_turn_state = attempt_headers.turn_state();
            let incoming_conversation_id = attempt_headers.conversation_id();
            let prompt_cache_key_for_trace = extract_prompt_cache_key_for_trace(&body_for_attempt);
            super::super::super::trace_log::log_attempt_profile(
                trace_id,
                &account.id,
                idx,
                setup.candidate_count,
                strip_session_affinity,
                incoming_session_id.is_some() || setup.has_sticky_fallback_session,
                incoming_turn_state.is_some(),
                incoming_conversation_id.is_some() || setup.has_sticky_fallback_conversation,
                prompt_cache_key_for_trace.as_deref(),
                request_shape,
                body_for_attempt.len(),
                attempt_model_for_log,
            );

            let mut inflight_guard =
                Some(super::super::super::acquire_account_inflight(&account.id));
            let mut attempt_trace = CandidateAttemptTrace::default();
            let decision = run_candidate_attempt(CandidateAttemptParams {
                storage,
                method,
                request_ctx,
                incoming_headers: &attempt_headers,
                body: &body_for_attempt,
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
                has_more_candidates: context.has_more_candidates(idx),
                context,
                setup,
                trace: &mut attempt_trace,
            });

            match decision {
                CandidateUpstreamDecision::Failover => {
                    if setup.records_persistent_affinity() {
                        super::super::super::affinity::record_affinity_attempt_feedback(
                            &account.id,
                            attempt_trace.last_status_code.unwrap_or(502),
                            attempt_trace.last_attempt_error.as_deref(),
                        );
                    }
                    super::super::super::record_gateway_failover_attempt();
                    last_attempt_url = attempt_trace.last_attempt_url.take();
                    last_attempt_error = attempt_trace.last_attempt_error.take();
                    continue;
                }
                CandidateUpstreamDecision::Terminal {
                    status_code,
                    message,
                } => {
                    if setup.records_persistent_affinity() {
                        super::super::super::affinity::record_affinity_attempt_feedback(
                            &account.id,
                            status_code,
                            Some(message.as_str()),
                        );
                    }
                    let request = request
                        .take()
                        .expect("request should be available before terminal response");
                    finalize_terminal_candidate(
                        request,
                        context,
                        &account.id,
                        attempt_trace.last_attempt_url.as_deref(),
                        status_code,
                        message,
                        trace_id,
                        started_at,
                        attempt_model_for_log,
                        Some(attempted_account_ids.as_slice()),
                    )?;
                    return Ok(CandidateExecutionResult::Handled);
                }
                CandidateUpstreamDecision::RespondUpstream(mut resp) => {
                    let mut request_body_for_success = body_for_attempt.clone();
                    if resp.status().as_u16() == 400
                        && !strip_session_affinity
                        && (incoming_turn_state.is_some() || setup.has_body_encrypted_content)
                    {
                        let retry_body = state.retry_body(
                            path,
                            body,
                            setup,
                            persistent_body_override.as_ref(),
                            attempt_model_override.as_deref(),
                            attempt_thread_anchor.as_deref(),
                        );
                        let retry_decision = run_candidate_attempt(CandidateAttemptParams {
                            storage,
                            method,
                            request_ctx,
                            incoming_headers: &attempt_headers,
                            body: &retry_body,
                            client_is_stream,
                            upstream_is_stream,
                            path,
                            request_deadline,
                            account,
                            token,
                            strip_session_affinity: true,
                            debug,
                            allow_openai_fallback,
                            disable_challenge_stateless_retry,
                            has_more_candidates: context.has_more_candidates(idx),
                            context,
                            setup,
                            trace: &mut attempt_trace,
                        });

                        match retry_decision {
                            CandidateUpstreamDecision::RespondUpstream(retry_resp) => {
                                resp = retry_resp;
                                request_body_for_success = retry_body.clone();
                            }
                            CandidateUpstreamDecision::Failover => {
                                if setup.records_persistent_affinity() {
                                    super::super::super::affinity::record_affinity_attempt_feedback(
                                        &account.id,
                                        attempt_trace.last_status_code.unwrap_or(502),
                                        attempt_trace.last_attempt_error.as_deref(),
                                    );
                                }
                                super::super::super::record_gateway_failover_attempt();
                                last_attempt_url = attempt_trace.last_attempt_url.take();
                                last_attempt_error = attempt_trace.last_attempt_error.take();
                                continue;
                            }
                            CandidateUpstreamDecision::Terminal {
                                status_code,
                                message,
                            } => {
                                if setup.records_persistent_affinity() {
                                    super::super::super::affinity::record_affinity_attempt_feedback(
                                        &account.id,
                                        status_code,
                                        Some(message.as_str()),
                                    );
                                }
                                let request = request
                                    .take()
                                    .expect("request should be available before terminal response");
                                finalize_terminal_candidate(
                                    request,
                                    context,
                                    &account.id,
                                    attempt_trace.last_attempt_url.as_deref(),
                                    status_code,
                                    message,
                                    trace_id,
                                    started_at,
                                    attempt_model_for_log,
                                    Some(attempted_account_ids.as_slice()),
                                )?;
                                return Ok(CandidateExecutionResult::Handled);
                            }
                        }
                    }
                    let request = request
                        .take()
                        .expect("request should be available before terminal response");
                    let guard = inflight_guard
                        .take()
                        .expect("inflight guard should be available before terminal response");
                    if setup.records_legacy_conversation_binding() {
                        if let Err(err) = super::super::super::conversation_binding::record_conversation_binding_terminal_response(
                            storage,
                            setup.legacy_conversation_routing(),
                            account,
                            attempt_model_for_log,
                            resp.status().as_u16(),
                        ) {
                            log::warn!(
                                "event=gateway_conversation_binding_update_failed trace_id={} account_id={} err={}",
                                trace_id,
                                account.id,
                                err
                            );
                        }
                    }
                    finalize_upstream_response(
                        request,
                        resp,
                        guard,
                        context,
                        account,
                        &request_body_for_success,
                        attempt_trace.last_attempt_url.as_deref(),
                        attempt_trace.last_attempt_error.as_deref(),
                        response_adapter,
                        tool_name_restore_map,
                        client_is_stream,
                        path,
                        attempt_affinity_resolution.as_ref(),
                        setup.peer_runtime_key.as_deref(),
                        trace_id,
                        started_at,
                        attempt_model_for_log,
                        Some(attempted_account_ids.as_slice()),
                    )?;
                    return Ok(CandidateExecutionResult::Handled);
                }
            }
        }

        if skipped_this_round > 0 {
            if super::super::super::wait_for_scheduler_candidate_window(
                candidates.as_slice(),
                &setup.account_dynamic_limits,
                request_deadline,
            ) {
                continue;
            }
        }

        break;
    }

    Ok(CandidateExecutionResult::Exhausted {
        request: request
            .expect("request should still exist when no candidate handled the response"),
        attempted_account_ids,
        skipped_cooldown,
        skipped_inflight,
        last_attempt_url,
        last_attempt_error,
    })
}
