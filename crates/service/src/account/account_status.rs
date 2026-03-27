use codexmanager_core::storage::{now_ts, Event, Storage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccountAvailabilitySignal {
    RefreshToken(crate::usage_http::RefreshTokenAuthErrorReason),
    Deactivation(&'static str),
    UsageHttp(u16),
}

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

pub(crate) fn classify_account_availability_signal(err: &str) -> Option<AccountAvailabilitySignal> {
    if let Some(reason) = crate::usage_http::refresh_token_auth_error_reason_from_message(err) {
        return Some(AccountAvailabilitySignal::RefreshToken(reason));
    }
    if let Some(reason) = deactivation_reason_from_message(err) {
        return Some(AccountAvailabilitySignal::Deactivation(reason));
    }
    if let Some(status_code) = extract_usage_http_status_code(err) {
        return Some(AccountAvailabilitySignal::UsageHttp(status_code));
    }
    None
}

fn extract_usage_http_status_code(message: &str) -> Option<u16> {
    let rest = message.trim().strip_prefix("usage endpoint status ")?;
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u16>().ok()
}

pub(crate) fn deactivation_reason_from_message(message: &str) -> Option<&'static str> {
    let normalized = message.trim().to_ascii_lowercase();
    if normalized.contains("workspace_deactivated")
        || normalized.contains("deactivated_workspace")
        || normalized.contains("workspace deactivated")
        || normalized.contains("workspace-deactivated")
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

pub(crate) fn is_banned_status_reason(reason: &str) -> bool {
    matches!(
        reason.trim().to_ascii_lowercase().as_str(),
        "account_deactivated" | "workspace_deactivated" | "deactivated_workspace"
    )
}

fn set_account_unavailable_with_reason(storage: &Storage, account_id: &str, reason: &str) -> bool {
    if should_preserve_manual_account_status(storage, account_id) {
        return false;
    }
    set_account_status(storage, account_id, "unavailable", reason);
    true
}

pub(crate) fn mark_account_unavailable_for_usage_http_error(
    storage: &Storage,
    account_id: &str,
    err: &str,
) -> bool {
    let Some(AccountAvailabilitySignal::UsageHttp(status_code)) =
        classify_account_availability_signal(err)
    else {
        return false;
    };
    match status_code {
        401 | 403 | 429 => {
            let status_reason = format!("usage_http_{status_code}");
            set_account_unavailable_with_reason(storage, account_id, &status_reason)
        }
        _ => false,
    }
}

pub(crate) fn mark_account_unavailable_for_deactivation_error(
    storage: &Storage,
    account_id: &str,
    err: &str,
) -> bool {
    let Some(AccountAvailabilitySignal::Deactivation(reason)) =
        classify_account_availability_signal(err)
    else {
        return false;
    };
    set_account_unavailable_with_reason(storage, account_id, reason)
}

pub(crate) fn mark_account_unavailable_for_auth_error(
    storage: &Storage,
    account_id: &str,
    err: &str,
) -> bool {
    let Some(signal) = classify_account_availability_signal(err) else {
        return false;
    };
    match signal {
        AccountAvailabilitySignal::RefreshToken(reason) => {
            let status_reason = format!("refresh_token_invalid:{}", reason.as_code());
            set_account_unavailable_with_reason(storage, account_id, &status_reason)
        }
        AccountAvailabilitySignal::Deactivation(reason) => {
            set_account_unavailable_with_reason(storage, account_id, reason)
        }
        AccountAvailabilitySignal::UsageHttp(_) => false,
    }
}

pub(crate) fn mark_account_unavailable_for_refresh_token_error(
    storage: &Storage,
    account_id: &str,
    err: &str,
) -> bool {
    let Some(AccountAvailabilitySignal::RefreshToken(reason)) =
        classify_account_availability_signal(err)
    else {
        return false;
    };
    let status_reason = format!("refresh_token_invalid:{}", reason.as_code());
    set_account_unavailable_with_reason(storage, account_id, &status_reason)
}

#[cfg(test)]
mod tests {
    use super::{classify_account_availability_signal, AccountAvailabilitySignal};

    #[test]
    fn classify_account_availability_signal_separates_usage_refresh_and_deactivation() {
        assert!(matches!(
            classify_account_availability_signal("usage endpoint status 401 Unauthorized"),
            Some(AccountAvailabilitySignal::UsageHttp(401))
        ));
        assert!(matches!(
            classify_account_availability_signal("usage endpoint status 403 Forbidden"),
            Some(AccountAvailabilitySignal::UsageHttp(403))
        ));
        assert!(matches!(
            classify_account_availability_signal("usage endpoint status 429 Too Many Requests"),
            Some(AccountAvailabilitySignal::UsageHttp(429))
        ));

        assert!(matches!(
            classify_account_availability_signal(
                "refresh token failed with status 401 Unauthorized: Your access token could not be refreshed because your refresh token was revoked. Please log out and sign in again."
            ),
            Some(AccountAvailabilitySignal::RefreshToken(
                crate::usage_http::RefreshTokenAuthErrorReason::Invalidated
            ))
        ));

        assert!(matches!(
            classify_account_availability_signal("account_deactivated"),
            Some(AccountAvailabilitySignal::Deactivation(
                "account_deactivated"
            ))
        ));
    }
}
