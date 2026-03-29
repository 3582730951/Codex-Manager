use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::OnceLock;

use super::IncomingHeaderSnapshot;

mod routing;

pub(crate) use routing::{
    acquire_affinity_lock, acquire_conversation_lock, finalize_affinity_success,
    record_affinity_attempt_feedback, resolve_enforced_routing, AffinityRoutingResolution,
};

const ENV_AFFINITY_ROUTING_MODE: &str = "CODEXMANAGER_AFFINITY_ROUTING_MODE";
const ENV_CONTEXT_REPLAY_ENABLED: &str = "CODEXMANAGER_CONTEXT_REPLAY_ENABLED";
const ENV_AFFINITY_SOFT_QUOTA_PERCENT: &str = "CODEXMANAGER_AFFINITY_SOFT_QUOTA_PERCENT";
const ENV_REPLAY_MAX_TURNS: &str = "CODEXMANAGER_REPLAY_MAX_TURNS";
const DEFAULT_AFFINITY_SOFT_QUOTA_PERCENT: u64 = 5;
const DEFAULT_REPLAY_MAX_TURNS: u64 = 12;

const MODE_OFF: u8 = 0;
const MODE_OBSERVE: u8 = 1;
const MODE_ENFORCE: u8 = 2;

static AFFINITY_MODE: AtomicU8 = AtomicU8::new(MODE_OFF);
static CONTEXT_REPLAY_ENABLED: AtomicBool = AtomicBool::new(true);
static AFFINITY_SOFT_QUOTA_PERCENT: AtomicU64 =
    AtomicU64::new(DEFAULT_AFFINITY_SOFT_QUOTA_PERCENT);
static REPLAY_MAX_TURNS: AtomicU64 = AtomicU64::new(DEFAULT_REPLAY_MAX_TURNS);
static AFFINITY_RUNTIME_LOADED: OnceLock<()> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AffinityRoutingMode {
    Off,
    Observe,
    Enforce,
}

impl AffinityRoutingMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Observe => "observe",
            Self::Enforce => "enforce",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DerivedAffinityKey {
    pub(crate) key: String,
    pub(crate) source: &'static str,
}

pub(crate) fn reload_from_env() {
    AFFINITY_MODE.store(
        parse_affinity_mode(std::env::var(ENV_AFFINITY_ROUTING_MODE).ok().as_deref())
            .map(mode_to_u8)
            .unwrap_or(MODE_OFF),
        Ordering::Relaxed,
    );
    CONTEXT_REPLAY_ENABLED.store(
        env_bool_or(ENV_CONTEXT_REPLAY_ENABLED, true),
        Ordering::Relaxed,
    );
    AFFINITY_SOFT_QUOTA_PERCENT.store(
        env_u64_or(
            ENV_AFFINITY_SOFT_QUOTA_PERCENT,
            DEFAULT_AFFINITY_SOFT_QUOTA_PERCENT,
        )
        .min(100),
        Ordering::Relaxed,
    );
    REPLAY_MAX_TURNS.store(
        env_u64_or(ENV_REPLAY_MAX_TURNS, DEFAULT_REPLAY_MAX_TURNS).max(1),
        Ordering::Relaxed,
    );
}

pub(crate) fn current_mode() -> AffinityRoutingMode {
    ensure_loaded();
    u8_to_mode(AFFINITY_MODE.load(Ordering::Relaxed))
}

pub(crate) fn set_mode(raw: &str) -> Result<AffinityRoutingMode, String> {
    ensure_loaded();
    let mode = parse_affinity_mode(Some(raw))
        .ok_or_else(|| "invalid affinity routing mode; use off, observe, or enforce".to_string())?;
    AFFINITY_MODE.store(mode_to_u8(mode), Ordering::Relaxed);
    std::env::set_var(ENV_AFFINITY_ROUTING_MODE, mode.as_str());
    Ok(mode)
}

pub(crate) fn context_replay_enabled() -> bool {
    ensure_loaded();
    CONTEXT_REPLAY_ENABLED.load(Ordering::Relaxed)
}

pub(crate) fn set_context_replay_enabled(enabled: bool) -> bool {
    ensure_loaded();
    CONTEXT_REPLAY_ENABLED.store(enabled, Ordering::Relaxed);
    std::env::set_var(ENV_CONTEXT_REPLAY_ENABLED, if enabled { "1" } else { "0" });
    enabled
}

pub(crate) fn current_affinity_soft_quota_percent() -> u64 {
    ensure_loaded();
    AFFINITY_SOFT_QUOTA_PERCENT.load(Ordering::Relaxed)
}

pub(crate) fn set_affinity_soft_quota_percent(value: u64) -> Result<u64, String> {
    ensure_loaded();
    if value > 100 {
        return Err("affinity soft quota percent must be between 0 and 100".to_string());
    }
    AFFINITY_SOFT_QUOTA_PERCENT.store(value, Ordering::Relaxed);
    std::env::set_var(ENV_AFFINITY_SOFT_QUOTA_PERCENT, value.to_string());
    Ok(value)
}

pub(crate) fn current_replay_max_turns() -> u64 {
    ensure_loaded();
    REPLAY_MAX_TURNS.load(Ordering::Relaxed)
}

pub(crate) fn set_replay_max_turns(value: u64) -> Result<u64, String> {
    ensure_loaded();
    if value == 0 {
        return Err("replay max turns must be greater than 0".to_string());
    }
    REPLAY_MAX_TURNS.store(value, Ordering::Relaxed);
    std::env::set_var(ENV_REPLAY_MAX_TURNS, value.to_string());
    Ok(value)
}

pub(crate) fn derive_affinity_key(
    incoming_headers: &IncomingHeaderSnapshot,
    local_conversation_id: Option<&str>,
) -> Option<DerivedAffinityKey> {
    derive_affinity_key_from_parts(
        incoming_headers.cli_affinity_id(),
        local_conversation_id.or(incoming_headers.conversation_id()),
        incoming_headers.session_id(),
        incoming_headers.subagent(),
        incoming_headers.client_request_id(),
    )
}

pub(crate) fn derive_affinity_key_from_parts(
    cli_affinity_id: Option<&str>,
    conversation_id: Option<&str>,
    session_id: Option<&str>,
    subagent: Option<&str>,
    client_request_id: Option<&str>,
) -> Option<DerivedAffinityKey> {
    if let Some(value) = normalize_key_part(cli_affinity_id) {
        return Some(DerivedAffinityKey {
            key: format!("cli:{value}"),
            source: "x-codex-cli-affinity-id",
        });
    }
    if let Some(value) = normalize_key_part(conversation_id) {
        return Some(DerivedAffinityKey {
            key: format!("cid:{value}"),
            source: "conversation_id",
        });
    }
    if let Some(value) = normalize_key_part(session_id) {
        return Some(DerivedAffinityKey {
            key: format!("sid:{value}"),
            source: "session_id",
        });
    }
    if let Some(value) = normalize_key_part(subagent) {
        return Some(DerivedAffinityKey {
            key: format!("sub:{value}"),
            source: "x-openai-subagent",
        });
    }
    normalize_key_part(client_request_id).map(|value| DerivedAffinityKey {
        key: format!("rid:{value}"),
        source: "x-client-request-id",
    })
}

pub(crate) fn derive_compat_affinity_keys(
    incoming_headers: &IncomingHeaderSnapshot,
    local_conversation_id: Option<&str>,
) -> Vec<DerivedAffinityKey> {
    let conversation_id = local_conversation_id.or(incoming_headers.conversation_id());
    let candidates = [
        (
            normalize_key_part(incoming_headers.session_id()),
            "sid",
            "session_id",
        ),
        (normalize_key_part(conversation_id), "cid", "conversation_id"),
        (
            normalize_key_part(incoming_headers.subagent()),
            "sub",
            "x-openai-subagent",
        ),
        (
            normalize_key_part(incoming_headers.client_request_id()),
            "rid",
            "x-client-request-id",
        ),
    ];
    let primary = derive_affinity_key(incoming_headers, local_conversation_id).map(|item| item.key);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (value, prefix, source) in candidates {
        let Some(value) = value else {
            continue;
        };
        let key = format!("{prefix}:{value}");
        if primary.as_deref() == Some(key.as_str()) || !seen.insert(key.clone()) {
            continue;
        }
        out.push(DerivedAffinityKey { key, source });
    }
    out
}

pub(crate) fn legacy_conversation_lock_key(conversation_id: Option<&str>) -> Option<String> {
    normalize_key_part(conversation_id).map(|value| format!("legacy-cid:{value}"))
}

pub(crate) fn derive_affinity_lock_keys(
    incoming_headers: &IncomingHeaderSnapshot,
    local_conversation_id: Option<&str>,
) -> Vec<String> {
    let conversation_id = local_conversation_id.or(incoming_headers.conversation_id());
    let mut keys = BTreeSet::new();
    if let Some(derived) = derive_affinity_key(incoming_headers, local_conversation_id) {
        keys.insert(derived.key);
    }
    for candidate in derive_compat_affinity_keys(incoming_headers, local_conversation_id) {
        keys.insert(candidate.key);
    }
    if let Some(legacy_key) = legacy_conversation_lock_key(conversation_id) {
        keys.insert(legacy_key);
    }
    keys.into_iter().collect()
}

pub(crate) fn synthetic_scope_id(platform_key_hash: &str, affinity_key: &str) -> String {
    let digest = Sha256::digest(format!("{platform_key_hash}:{affinity_key}").as_bytes());
    format!(
        "affinity::{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7]
    )
}

pub(crate) fn derive_thread_anchor(
    platform_key_hash: &str,
    affinity_key: &str,
    conversation_scope_id: &str,
    thread_epoch: i64,
) -> String {
    let digest = Sha256::digest(
        format!("{platform_key_hash}:{affinity_key}:{conversation_scope_id}:{thread_epoch}")
            .as_bytes(),
    );
    format!(
        "cmgr-aff-{}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        thread_epoch,
        digest[0],
        digest[1],
        digest[2],
        digest[3],
        digest[4],
        digest[5],
        digest[6],
        digest[7]
    )
}

fn ensure_loaded() {
    let _ = AFFINITY_RUNTIME_LOADED.get_or_init(|| reload_from_env());
}

fn normalize_key_part(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn parse_affinity_mode(raw: Option<&str>) -> Option<AffinityRoutingMode> {
    match raw.map(str::trim).unwrap_or_default().to_ascii_lowercase().as_str() {
        "" | "off" | "disabled" => Some(AffinityRoutingMode::Off),
        "observe" | "read_only" | "readonly" => Some(AffinityRoutingMode::Observe),
        "enforce" | "on" | "enabled" => Some(AffinityRoutingMode::Enforce),
        _ => None,
    }
}

fn mode_to_u8(mode: AffinityRoutingMode) -> u8 {
    match mode {
        AffinityRoutingMode::Off => MODE_OFF,
        AffinityRoutingMode::Observe => MODE_OBSERVE,
        AffinityRoutingMode::Enforce => MODE_ENFORCE,
    }
}

fn u8_to_mode(value: u8) -> AffinityRoutingMode {
    match value {
        MODE_OBSERVE => AffinityRoutingMode::Observe,
        MODE_ENFORCE => AffinityRoutingMode::Enforce,
        _ => AffinityRoutingMode::Off,
    }
}

fn env_u64_or(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_bool_or(name: &str, default: bool) -> bool {
    let Some(value) = std::env::var(name).ok() else {
        return default;
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        derive_affinity_key_from_parts, legacy_conversation_lock_key, synthetic_scope_id,
        AffinityRoutingMode,
    };

    #[test]
    fn derive_affinity_key_prefers_cli_then_conversation_then_session() {
        let derived = derive_affinity_key_from_parts(
            Some("cli-1"),
            Some("conv-1"),
            Some("sid-1"),
            Some("sub-1"),
            Some("req-1"),
        )
        .expect("derive affinity key");

        assert_eq!(derived.key, "cli:cli-1");
        assert_eq!(derived.source, "x-codex-cli-affinity-id");
    }

    #[test]
    fn derive_affinity_key_falls_back_to_conversation_id_before_session_id() {
        let derived = derive_affinity_key_from_parts(
            None,
            Some("conv-1"),
            Some("sid-1"),
            Some("sub-1"),
            Some("req-1"),
        )
        .expect("derive affinity key");

        assert_eq!(derived.key, "cid:conv-1");
        assert_eq!(derived.source, "conversation_id");
    }

    #[test]
    fn legacy_conversation_lock_key_is_stable() {
        assert_eq!(
            legacy_conversation_lock_key(Some("conv-1")).as_deref(),
            Some("legacy-cid:conv-1")
        );
    }

    #[test]
    fn synthetic_scope_id_is_stable() {
        assert_eq!(
            synthetic_scope_id("key-hash-1", "cid:conv-1"),
            synthetic_scope_id("key-hash-1", "cid:conv-1")
        );
    }

    #[test]
    fn affinity_mode_labels_match_expected_strings() {
        assert_eq!(AffinityRoutingMode::Off.as_str(), "off");
        assert_eq!(AffinityRoutingMode::Observe.as_str(), "observe");
        assert_eq!(AffinityRoutingMode::Enforce.as_str(), "enforce");
    }
}
