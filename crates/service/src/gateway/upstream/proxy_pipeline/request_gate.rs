use super::super::support::deadline;
use std::time::{Duration, Instant};

pub(in super::super) fn acquire_request_gate(
    trace_id: &str,
    key_id: &str,
    account_id: &str,
    path: &str,
    model_for_log: Option<&str>,
    is_stream: bool,
    request_deadline: Option<Instant>,
) -> Option<super::super::super::request_gate::RequestGateGuard> {
    let request_gate_lock =
        super::super::super::request_gate_lock(key_id, Some(account_id), path, model_for_log);
    let request_gate_wait_timeout =
        super::super::super::runtime_config::request_gate_wait_timeout_for(path, is_stream);
    super::super::super::trace_log::log_request_gate_wait(
        trace_id,
        key_id,
        account_id,
        path,
        model_for_log,
        request_gate_wait_timeout.as_millis(),
    );
    let gate_wait_started_at = Instant::now();

    match request_gate_lock.try_acquire() {
        Ok(Some(guard)) => {
            super::super::super::trace_log::log_request_gate_acquired(
                trace_id,
                key_id,
                account_id,
                path,
                model_for_log,
                0,
            );
            Some(guard)
        }
        Ok(None) => {
            let effective_wait = deadline::cap_wait(request_gate_wait_timeout, request_deadline)
                .unwrap_or(Duration::from_millis(0));
            let wait_result = if effective_wait.is_zero() {
                Ok(None)
            } else {
                request_gate_lock.acquire_with_timeout(effective_wait)
            };
            if let Ok(Some(guard)) = wait_result {
                super::super::super::trace_log::log_request_gate_acquired(
                    trace_id,
                    key_id,
                    account_id,
                    path,
                    model_for_log,
                    gate_wait_started_at.elapsed().as_millis(),
                );
                Some(guard)
            } else {
                match wait_result {
                    Err(super::super::super::RequestGateAcquireError::Poisoned) => {
                        super::super::super::trace_log::log_request_gate_skip(
                            trace_id,
                            account_id,
                            "lock_poisoned",
                        );
                    }
                    _ => {
                        let reason = if deadline::is_expired(request_deadline) {
                            "total_timeout"
                        } else {
                            "gate_wait_timeout"
                        };
                        super::super::super::trace_log::log_request_gate_skip(
                            trace_id, account_id, reason,
                        );
                    }
                }
                None
            }
        }
        Err(super::super::super::RequestGateAcquireError::Poisoned) => {
            super::super::super::trace_log::log_request_gate_skip(
                trace_id,
                account_id,
                "lock_poisoned",
            );
            None
        }
    }
}
