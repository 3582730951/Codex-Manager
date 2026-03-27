use std::time::Instant;

pub(in super::super) fn acquire_request_gate(
    trace_id: &str,
    key_id: &str,
    path: &str,
    model_for_log: Option<&str>,
    request_deadline: Option<Instant>,
) -> Option<super::super::super::request_gate::RequestGateGuard> {
    let request_gate_lock = super::super::super::request_gate_lock(key_id, path, model_for_log);
    super::super::super::trace_log::log_request_gate_wait(trace_id, key_id, path, model_for_log);

    match request_gate_lock.try_acquire() {
        Ok(Some(guard)) => {
            super::super::super::trace_log::log_request_gate_acquired(
                trace_id,
                key_id,
                path,
                model_for_log,
                0,
            );
            Some(guard)
        }
        Ok(None) => {
            let reason = if request_deadline.is_some_and(|deadline| deadline <= Instant::now()) {
                "total_timeout"
            } else {
                "gate_busy"
            };
            super::super::super::trace_log::log_request_gate_skip(trace_id, reason);
            None
        }
        Err(super::super::super::RequestGateAcquireError::Poisoned) => {
            super::super::super::trace_log::log_request_gate_skip(trace_id, "lock_poisoned");
            None
        }
    }
}
