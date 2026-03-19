use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const DEFAULT_MODEL_INELIGIBLE_TTL_SECS: u64 = 30 * 60;
const DEFAULT_QUOTA_REJECTED_TTL_SECS: u64 = 10 * 60;
const ENV_MODEL_INELIGIBLE_TTL_SECS: &str = "CODEXMANAGER_MODEL_INELIGIBLE_TTL_SECS";
const ENV_QUOTA_REJECTED_TTL_SECS: &str = "CODEXMANAGER_QUOTA_REJECTED_TTL_SECS";

static MODEL_INELIGIBLE_TTL_SECS: AtomicU64 =
    AtomicU64::new(DEFAULT_MODEL_INELIGIBLE_TTL_SECS);
static QUOTA_REJECTED_TTL_SECS: AtomicU64 = AtomicU64::new(DEFAULT_QUOTA_REJECTED_TTL_SECS);
static MODEL_FEEDBACK_CONFIG_LOADED: OnceLock<()> = OnceLock::new();
static MODEL_FEEDBACK_STATE: OnceLock<Mutex<ModelFeedbackState>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccountRequestFeedback {
    ModelIneligible,
    QuotaRejected,
}

#[derive(Clone, Copy)]
struct FeedbackEntry {
    reason: AccountRequestFeedback,
    expires_at: Instant,
}

#[derive(Default)]
struct ModelFeedbackState {
    per_request: HashMap<(String, String), FeedbackEntry>,
    per_account: HashMap<String, FeedbackEntry>,
}

pub(crate) fn reload_from_env() {
    MODEL_INELIGIBLE_TTL_SECS.store(
        env_u64_or(
            ENV_MODEL_INELIGIBLE_TTL_SECS,
            DEFAULT_MODEL_INELIGIBLE_TTL_SECS,
        ),
        Ordering::Relaxed,
    );
    QUOTA_REJECTED_TTL_SECS.store(
        env_u64_or(ENV_QUOTA_REJECTED_TTL_SECS, DEFAULT_QUOTA_REJECTED_TTL_SECS),
        Ordering::Relaxed,
    );
}

pub(crate) fn record_model_ineligible_feedback(account_id: &str, request_model: &str) {
    ensure_model_feedback_loaded();
    let Some(account_id) = normalize_key(account_id) else {
        return;
    };
    let Some(request_model) = normalize_key(request_model) else {
        return;
    };
    let expires_at = Instant::now() + Duration::from_secs(model_ineligible_ttl_secs().max(1));
    let lock = MODEL_FEEDBACK_STATE.get_or_init(|| Mutex::new(ModelFeedbackState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "model_feedback_state");
    state.cleanup_expired(Instant::now());
    state.per_request.insert(
        (account_id, request_model),
        FeedbackEntry {
            reason: AccountRequestFeedback::ModelIneligible,
            expires_at,
        },
    );
}

pub(crate) fn record_quota_rejected_feedback(account_id: &str, request_model: Option<&str>) {
    ensure_model_feedback_loaded();
    let Some(account_id) = normalize_key(account_id) else {
        return;
    };
    let expires_at = Instant::now() + Duration::from_secs(quota_rejected_ttl_secs().max(1));
    let lock = MODEL_FEEDBACK_STATE.get_or_init(|| Mutex::new(ModelFeedbackState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "model_feedback_state");
    state.cleanup_expired(Instant::now());
    state.per_account.insert(
        account_id.clone(),
        FeedbackEntry {
            reason: AccountRequestFeedback::QuotaRejected,
            expires_at,
        },
    );
    if let Some(request_model) = request_model.and_then(normalize_key) {
        state.per_request.insert(
            (account_id, request_model),
            FeedbackEntry {
                reason: AccountRequestFeedback::QuotaRejected,
                expires_at,
            },
        );
    }
}

pub(crate) fn clear_request_feedback(account_id: &str, request_model: Option<&str>) {
    ensure_model_feedback_loaded();
    let Some(account_id) = normalize_key(account_id) else {
        return;
    };
    let lock = MODEL_FEEDBACK_STATE.get_or_init(|| Mutex::new(ModelFeedbackState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "model_feedback_state");
    state.cleanup_expired(Instant::now());
    state.per_account.remove(account_id.as_str());
    if let Some(request_model) = request_model.and_then(normalize_key) {
        state.per_request.remove(&(account_id, request_model));
    }
}

pub(crate) fn request_feedback_for(
    account_id: &str,
    request_model: Option<&str>,
) -> Option<AccountRequestFeedback> {
    ensure_model_feedback_loaded();
    let account_id = normalize_key(account_id)?;
    let lock = MODEL_FEEDBACK_STATE.get_or_init(|| Mutex::new(ModelFeedbackState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "model_feedback_state");
    state.cleanup_expired(Instant::now());
    if let Some(request_model) = request_model.and_then(normalize_key) {
        if let Some(entry) = state.per_request.get(&(account_id.clone(), request_model)) {
            return Some(entry.reason);
        }
    }
    state.per_account.get(account_id.as_str()).map(|entry| entry.reason)
}

fn ensure_model_feedback_loaded() {
    let _ = MODEL_FEEDBACK_CONFIG_LOADED.get_or_init(|| reload_from_env());
}

fn model_ineligible_ttl_secs() -> u64 {
    MODEL_INELIGIBLE_TTL_SECS.load(Ordering::Relaxed)
}

fn quota_rejected_ttl_secs() -> u64 {
    QUOTA_REJECTED_TTL_SECS.load(Ordering::Relaxed)
}

fn env_u64_or(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn normalize_key(raw: &str) -> Option<String> {
    let normalized = raw.trim().to_ascii_lowercase();
    (!normalized.is_empty()).then_some(normalized)
}

impl ModelFeedbackState {
    fn cleanup_expired(&mut self, now: Instant) {
        self.per_request.retain(|_, entry| entry.expires_at > now);
        self.per_account.retain(|_, entry| entry.expires_at > now);
    }
}

#[cfg(test)]
pub(crate) fn clear_runtime_state_for_tests() {
    reload_from_env();
    let lock = MODEL_FEEDBACK_STATE.get_or_init(|| Mutex::new(ModelFeedbackState::default()));
    let mut state = crate::lock_utils::lock_recover(lock, "model_feedback_state");
    state.per_request.clear();
    state.per_account.clear();
}

#[cfg(test)]
mod tests {
    use super::{
        clear_request_feedback, clear_runtime_state_for_tests, record_model_ineligible_feedback,
        record_quota_rejected_feedback, request_feedback_for, AccountRequestFeedback,
    };

    #[test]
    fn model_feedback_is_scoped_by_request_model() {
        clear_runtime_state_for_tests();
        record_model_ineligible_feedback("acc-a", "gpt-5.4");
        assert_eq!(
            request_feedback_for("acc-a", Some("gpt-5.4")),
            Some(AccountRequestFeedback::ModelIneligible)
        );
        assert_eq!(request_feedback_for("acc-a", Some("gpt-5.3")), None);
    }

    #[test]
    fn quota_feedback_applies_account_wide() {
        clear_runtime_state_for_tests();
        record_quota_rejected_feedback("acc-b", Some("gpt-5.4"));
        assert_eq!(
            request_feedback_for("acc-b", Some("gpt-5.4")),
            Some(AccountRequestFeedback::QuotaRejected)
        );
        assert_eq!(
            request_feedback_for("acc-b", Some("gpt-5.3")),
            Some(AccountRequestFeedback::QuotaRejected)
        );
    }

    #[test]
    fn clearing_feedback_removes_both_request_and_account_entries() {
        clear_runtime_state_for_tests();
        record_quota_rejected_feedback("acc-c", Some("gpt-5.4"));
        clear_request_feedback("acc-c", Some("gpt-5.4"));
        assert_eq!(request_feedback_for("acc-c", Some("gpt-5.4")), None);
    }
}
