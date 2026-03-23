use codexmanager_core::storage::{now_ts, Event, Storage};

fn latest_status_reason(storage: &Storage, account_id: &str) -> Option<String> {
    storage
        .latest_account_status_reasons(&[account_id.to_string()])
        .ok()
        .and_then(|mut reasons| reasons.remove(account_id))
}

pub(crate) fn set_account_status(storage: &Storage, account_id: &str, status: &str, reason: &str) {
    let changed = matches!(
        storage.update_account_status_if_changed(account_id, status),
        Ok(true)
    );
    let account_exists = storage
        .find_account_by_id(account_id)
        .ok()
        .flatten()
        .is_some();
    if account_exists
        && (changed || latest_status_reason(storage, account_id).as_deref() != Some(reason))
    {
        let _ = storage.insert_event(&Event {
            account_id: Some(account_id.to_string()),
            event_type: "account_status_update".to_string(),
            message: format!("status={status} reason={reason}"),
            created_at: now_ts(),
        });
    }
}

fn should_preserve_manual_account_status(storage: &Storage, account_id: &str) -> bool {
    storage
        .find_account_by_id(account_id)
        .ok()
        .flatten()
        .map(|account| {
            account.status.trim().eq_ignore_ascii_case("disabled")
                || account.status.trim().eq_ignore_ascii_case("inactive")
        })
        .unwrap_or(false)
}

pub(crate) fn deactivation_reason_from_message(message: &str) -> Option<&'static str> {
    let normalized = message.trim().to_ascii_lowercase();
    if normalized.contains("workspace_deactivated")
        || normalized.contains("workspace deactivated")
        || normalized.contains("deactivated workspace")
    {
        return Some("workspace_deactivated");
    }
    if normalized.contains("account_deactivated")
        || normalized.contains("account deactivated")
        || normalized.contains("deactivated")
    {
        return Some("account_deactivated");
    }
    None
}

pub(crate) fn mark_account_unavailable_for_deactivation_error(
    storage: &Storage,
    account_id: &str,
    err: &str,
) -> bool {
    let Some(reason) = deactivation_reason_from_message(err) else {
        return false;
    };
    if should_preserve_manual_account_status(storage, account_id) {
        return false;
    }
    set_account_status(storage, account_id, "unavailable", reason);
    true
}

pub(crate) fn mark_account_unavailable_for_refresh_token_error(
    storage: &Storage,
    account_id: &str,
    err: &str,
) -> bool {
    let Some(reason) = crate::usage_http::refresh_token_auth_error_reason_from_message(err) else {
        return false;
    };
    if should_preserve_manual_account_status(storage, account_id) {
        return false;
    }
    let status_reason = format!("refresh_token_invalid:{}", reason.as_code());
    set_account_status(storage, account_id, "unavailable", &status_reason);
    true
}
