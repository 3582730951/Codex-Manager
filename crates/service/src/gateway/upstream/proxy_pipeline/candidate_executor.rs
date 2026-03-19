use bytes::Bytes;
use codexmanager_core::storage::{Account, Storage, Token};
use std::time::Duration;
use std::time::Instant;
use tiny_http::Request;

use super::super::attempt_flow::candidate_flow::CandidateUpstreamDecision;
use super::super::support::candidates::free_account_model_override;
use super::super::support::deadline;
use super::candidate_attempt::{
    run_candidate_attempt, CandidateAttemptParams, CandidateAttemptTrace,
};
use super::candidate_state::CandidateExecutionState;
use super::execution_context::GatewayUpstreamExecutionContext;
use super::request_setup::UpstreamRequestSetup;
use super::response_finalize::{
    finalize_terminal_candidate, finalize_upstream_response, respond_total_timeout,
};

pub(in super::super) enum CandidateExecutionResult {
    Handled,
    Exhausted(Request),
}

pub(in super::super) struct CandidateExecutorParams<'a> {
    pub(in super::super) storage: &'a Storage,
    pub(in super::super) method: &'a reqwest::Method,
    pub(in super::super) incoming_headers: &'a super::super::super::IncomingHeaderSnapshot,
    pub(in super::super) body: &'a Bytes,
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
    const MAX_CONSECUTIVE_CHALLENGE_FAILOVERS: usize = 2;

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
    let mut attempted_account_ids = Vec::new();
    let mut consecutive_challenge_failovers = 0usize;
    let mut candidates = candidates;
    let inflight_wait_timeout =
        super::super::super::runtime_config::account_inflight_wait_timeout_for(
            path,
            upstream_is_stream,
        );
    'contention_round: loop {
        let mut idx = 0usize;
        let mut saw_inflight_wait_timeout = false;
        while idx < candidates.len() {
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

            let account_id = candidates[idx].0.id.clone();
            let strip_session_affinity = state.strip_session_affinity(
                &candidates[idx].0,
                idx,
                setup.anthropic_has_prompt_cache_key,
            );
            let attempt_model_override =
                free_account_model_override(storage, &candidates[idx].0, &candidates[idx].1);
            let attempt_model_for_log = attempt_model_override.as_deref().or(model_for_log);
            let body_for_attempt = state.body_for_attempt(
                path,
                body,
                strip_session_affinity,
                setup,
                attempt_model_override.as_deref(),
            );
            context.log_candidate_start(&account_id, idx, strip_session_affinity);
            if let Some(skip_reason) = context.should_skip_candidate(&account_id, idx) {
                context.log_candidate_skip(&account_id, idx, skip_reason);
                idx += 1;
                continue;
            }
            // 中文注释：每个候选都单独拥有一份 inflight 等待预算。
            // 否则前一个忙候选消耗掉大部分预算后，后一个候选只拿到零头时间，
            // 很容易在“账号马上就空出来”的情况下仍然错误返回 503。
            let effective_inflight_wait =
                deadline::cap_wait(inflight_wait_timeout, request_deadline)
                    .unwrap_or(Duration::from_millis(0));
            if setup.account_max_inflight > 0
                && super::super::super::account_inflight_count(&account_id)
                    >= setup.account_max_inflight
            {
                let availability = remaining_inflight_availability(
                    candidates.as_slice(),
                    idx,
                    setup.account_max_inflight,
                );
                if availability.has_immediately_available_candidate {
                    context.log_candidate_skip_reason(&account_id, idx, "inflight");
                    idx += 1;
                    continue;
                }
                let slot_ready =
                    match super::super::super::metrics::wait_for_any_account_inflight_slot(
                        availability.busy_account_ids.as_slice(),
                        setup.account_max_inflight,
                        effective_inflight_wait,
                    ) {
                        Ok(ready) => ready,
                        Err(
                            super::super::super::metrics::AccountInFlightAcquireError::Poisoned,
                        ) => {
                            context.log_candidate_skip_reason(
                                &account_id,
                                idx,
                                "inflight_lock_poisoned",
                            );
                            idx += 1;
                            continue;
                        }
                    };
                if !slot_ready {
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
                    context.log_candidate_skip_reason(&account_id, idx, "inflight_wait_timeout");
                    saw_inflight_wait_timeout = true;
                    idx += 1;
                    continue;
                }
                continue;
            }
            attempted_account_ids.push(account_id.clone());

            let request_ref = request
                .as_ref()
                .ok_or_else(|| "request already consumed".to_string())?;
            let incoming_session_id = incoming_headers.session_id();
            let incoming_turn_state = incoming_headers.turn_state();
            let incoming_conversation_id = incoming_headers.conversation_id();
            super::super::super::trace_log::log_attempt_profile(
                trace_id,
                &account_id,
                idx,
                setup.candidate_count,
                strip_session_affinity,
                incoming_session_id.is_some() || setup.has_sticky_fallback_session,
                incoming_turn_state.is_some(),
                incoming_conversation_id.is_some() || setup.has_sticky_fallback_conversation,
                None,
                request_shape,
                body_for_attempt.len(),
                attempt_model_for_log,
            );

            let (account, token) = &mut candidates[idx];
            let mut inflight_guard =
                Some(super::super::super::acquire_account_inflight(&account.id));
            let mut attempt_trace = CandidateAttemptTrace::default();
            let decision = run_candidate_attempt(CandidateAttemptParams {
                storage,
                method,
                request: request_ref,
                incoming_headers,
                body: &body_for_attempt,
                upstream_is_stream,
                path,
                request_deadline,
                account: &account,
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
                    let challenge_failover = matches!(
                        attempt_trace.last_attempt_error.as_deref(),
                        Some("upstream challenge blocked")
                    );
                    if challenge_failover {
                        consecutive_challenge_failovers =
                            consecutive_challenge_failovers.saturating_add(1);
                        if consecutive_challenge_failovers >= MAX_CONSECUTIVE_CHALLENGE_FAILOVERS {
                            let request = request
                                .take()
                                .expect("request should be available before terminal response");
                            finalize_terminal_candidate(
                                request,
                                context,
                                &account.id,
                                attempt_trace.last_attempt_url.as_deref(),
                                attempt_trace.last_attempt_status_code.unwrap_or(403),
                                "upstream challenge blocked".to_string(),
                                trace_id,
                                started_at,
                                attempt_model_for_log,
                                Some(attempted_account_ids.as_slice()),
                            )?;
                            return Ok(CandidateExecutionResult::Handled);
                        }
                    } else {
                        consecutive_challenge_failovers = 0;
                    }
                    super::super::super::record_gateway_failover_attempt();
                    idx += 1;
                    continue;
                }
                CandidateUpstreamDecision::Terminal {
                    status_code,
                    message,
                } => {
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
                    if resp.status().as_u16() == 400
                        && !strip_session_affinity
                        && (incoming_turn_state.is_some() || setup.has_body_encrypted_content)
                    {
                        let retry_body =
                            state.retry_body(path, body, setup, attempt_model_override.as_deref());
                        let retry_decision = run_candidate_attempt(CandidateAttemptParams {
                            storage,
                            method,
                            request: request_ref,
                            incoming_headers,
                            body: &retry_body,
                            upstream_is_stream,
                            path,
                            request_deadline,
                            account: &account,
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
                            }
                            CandidateUpstreamDecision::Failover => {
                                super::super::super::record_gateway_failover_attempt();
                                idx += 1;
                                continue;
                            }
                            CandidateUpstreamDecision::Terminal {
                                status_code,
                                message,
                            } => {
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
                    finalize_upstream_response(
                        request,
                        resp,
                        guard,
                        context,
                        &account.id,
                        attempt_trace.last_attempt_url.as_deref(),
                        attempt_trace.last_attempt_error.as_deref(),
                        response_adapter,
                        tool_name_restore_map,
                        client_is_stream,
                        path,
                        trace_id,
                        started_at,
                        attempt_model_for_log,
                        Some(attempted_account_ids.as_slice()),
                    )?;
                    return Ok(CandidateExecutionResult::Handled);
                }
            }
        }

        // 中文注释：如果整轮都没有真正打到上游，只是因为所有候选都在 inflight 等待里超时，
        // 那就继续排队下一轮，直到请求 deadline，而不是过早返回 503。
        if attempted_account_ids.is_empty()
            && saw_inflight_wait_timeout
            && !deadline::is_expired(request_deadline)
        {
            continue 'contention_round;
        }

        break;
    }

    Ok(CandidateExecutionResult::Exhausted(request.expect(
        "request should still exist when no candidate handled the response",
    )))
}

struct RemainingInflightAvailability {
    busy_account_ids: Vec<String>,
    has_immediately_available_candidate: bool,
}

fn remaining_inflight_availability(
    candidates: &[(Account, Token)],
    start_idx: usize,
    account_max_inflight: usize,
) -> RemainingInflightAvailability {
    let mut busy_account_ids = Vec::new();
    let mut has_immediately_available_candidate = false;
    for (idx, (candidate_account, _)) in candidates.iter().enumerate().skip(start_idx) {
        if idx != start_idx && super::super::super::is_account_in_cooldown(&candidate_account.id) {
            continue;
        }
        if super::super::super::account_inflight_count(&candidate_account.id) < account_max_inflight
        {
            has_immediately_available_candidate = true;
            continue;
        }
        busy_account_ids.push(candidate_account.id.clone());
    }
    RemainingInflightAvailability {
        busy_account_ids,
        has_immediately_available_candidate,
    }
}

#[cfg(test)]
mod tests {
    use super::remaining_inflight_availability;
    use crate::gateway::acquire_account_inflight;
    use crate::gateway::metrics::clear_account_inflight_for_tests;
    use crate::gateway::{mark_account_cooldown, CooldownReason};
    use codexmanager_core::storage::{Account, Token};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn test_guard() -> MutexGuard<'static, ()> {
        static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
        TEST_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("candidate executor test mutex")
    }

    fn candidate(id: &str, sort: i64) -> (Account, Token) {
        (
            Account {
                id: id.to_string(),
                label: id.to_string(),
                issuer: "issuer".to_string(),
                chatgpt_account_id: None,
                workspace_id: None,
                group_name: None,
                sort,
                status: "active".to_string(),
                created_at: 0,
                updated_at: 0,
            },
            Token {
                account_id: id.to_string(),
                id_token: "id".to_string(),
                access_token: "access".to_string(),
                refresh_token: "refresh".to_string(),
                api_key_access_token: None,
                last_refresh: 0,
            },
        )
    }

    #[test]
    fn remaining_inflight_availability_reports_free_later_candidate() {
        let _guard = test_guard();
        clear_account_inflight_for_tests();
        let busy = acquire_account_inflight("acc-a");
        let candidates = vec![candidate("acc-a", 0), candidate("acc-b", 1)];

        let availability = remaining_inflight_availability(candidates.as_slice(), 0, 1);

        drop(busy);
        clear_account_inflight_for_tests();
        assert!(availability.has_immediately_available_candidate);
        assert_eq!(availability.busy_account_ids, vec!["acc-a".to_string()]);
    }

    #[test]
    fn remaining_inflight_availability_waits_when_all_remaining_candidates_are_busy() {
        let _guard = test_guard();
        clear_account_inflight_for_tests();
        let busy_a = acquire_account_inflight("acc-a");
        let busy_b = acquire_account_inflight("acc-b");
        let candidates = vec![candidate("acc-a", 0), candidate("acc-b", 1)];

        let availability = remaining_inflight_availability(candidates.as_slice(), 0, 1);

        drop(busy_b);
        drop(busy_a);
        clear_account_inflight_for_tests();
        assert!(!availability.has_immediately_available_candidate);
        assert_eq!(
            availability.busy_account_ids,
            vec!["acc-a".to_string(), "acc-b".to_string()]
        );
    }

    #[test]
    fn remaining_inflight_availability_ignores_cooldown_candidates() {
        let _guard = test_guard();
        clear_account_inflight_for_tests();
        let busy = acquire_account_inflight("acc-a");
        mark_account_cooldown("acc-b", CooldownReason::RateLimited);
        let candidates = vec![candidate("acc-a", 0), candidate("acc-b", 1)];

        let availability = remaining_inflight_availability(candidates.as_slice(), 0, 1);

        drop(busy);
        clear_account_inflight_for_tests();
        crate::gateway::clear_account_cooldown("acc-b");
        assert!(!availability.has_immediately_available_candidate);
        assert_eq!(availability.busy_account_ids, vec!["acc-a".to_string()]);
    }
}
