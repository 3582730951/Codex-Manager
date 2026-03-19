use super::*;
use crate::gateway::{build_codex_compact_upstream_headers, CodexCompactUpstreamHeaderInput};
use std::sync::{Mutex, MutexGuard};

static HEADER_RUNTIME_MUTEX: Mutex<()> = Mutex::new(());

fn header_runtime_guard() -> MutexGuard<'static, ()> {
    HEADER_RUNTIME_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct GatewayHeaderRuntimeRestore {
    originator: String,
    residency_requirement: Option<String>,
}

impl GatewayHeaderRuntimeRestore {
    fn capture() -> Self {
        Self {
            originator: crate::current_gateway_originator(),
            residency_requirement: crate::current_gateway_residency_requirement(),
        }
    }
}

impl Drop for GatewayHeaderRuntimeRestore {
    fn drop(&mut self) {
        let _ = crate::set_gateway_originator(&self.originator);
        let _ = crate::set_gateway_residency_requirement(self.residency_requirement.as_deref());
    }
}

fn find_header(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

#[test]
fn codex_header_profile_sets_required_headers_for_stream() {
    let headers = build_codex_upstream_headers(CodexUpstreamHeaderInput {
        auth_token: "token-123",
        account_id: Some("acc-1"),
        include_account_id: true,
        include_openai_beta: true,
        upstream_cookie: Some("cf_clearance=test"),
        incoming_session_id: None,
        incoming_client_request_id: Some("client-req-1"),
        incoming_subagent: Some("review"),
        fallback_session_id: None,
        incoming_turn_state: Some("turn-state"),
        include_turn_state: true,
        incoming_conversation_id: Some("conversation"),
        fallback_conversation_id: None,
        include_conversation_id: true,
        strip_session_affinity: false,
        is_stream: true,
        has_body: true,
    });

    assert_eq!(
        find_header(&headers, "Authorization").as_deref(),
        Some("Bearer token-123")
    );
    assert_eq!(
        find_header(&headers, "Content-Type").as_deref(),
        Some("application/json")
    );
    assert_eq!(
        find_header(&headers, "Accept").as_deref(),
        Some("text/event-stream")
    );
    assert_eq!(find_header(&headers, "Version").as_deref(), Some("0.101.0"));
    assert_eq!(
        find_header(&headers, "Openai-Beta").as_deref(),
        Some("responses=experimental")
    );
    assert_eq!(
        find_header(&headers, "Originator").as_deref(),
        Some("codex_cli_rs")
    );
    assert_eq!(
        find_header(&headers, "x-client-request-id").as_deref(),
        Some("client-req-1")
    );
    assert_eq!(
        find_header(&headers, "x-openai-subagent").as_deref(),
        Some("review")
    );
    assert_eq!(
        find_header(&headers, "Chatgpt-Account-Id").as_deref(),
        Some("acc-1")
    );
    assert_eq!(
        find_header(&headers, "Cookie").as_deref(),
        Some("cf_clearance=test")
    );
    assert_eq!(
        find_header(&headers, "x-codex-turn-state").as_deref(),
        Some("turn-state")
    );
    assert_eq!(
        find_header(&headers, "Conversation_id").as_deref(),
        Some("conversation")
    );
    assert!(find_header(&headers, "Session_id").is_some());
}

#[test]
fn codex_header_profile_uses_json_accept_for_non_stream() {
    let headers = build_codex_upstream_headers(CodexUpstreamHeaderInput {
        auth_token: "token-456",
        account_id: None,
        include_account_id: true,
        include_openai_beta: true,
        upstream_cookie: None,
        incoming_session_id: None,
        incoming_client_request_id: None,
        incoming_subagent: None,
        fallback_session_id: None,
        incoming_turn_state: None,
        include_turn_state: true,
        incoming_conversation_id: None,
        fallback_conversation_id: None,
        include_conversation_id: true,
        strip_session_affinity: false,
        is_stream: false,
        has_body: false,
    });

    assert_eq!(
        find_header(&headers, "Accept").as_deref(),
        Some("application/json")
    );
    assert!(find_header(&headers, "Content-Type").is_none());
}

#[test]
fn codex_compact_header_profile_matches_remote_compact_shape() {
    let headers = build_codex_compact_upstream_headers(CodexCompactUpstreamHeaderInput {
        auth_token: "token-compact",
        account_id: Some("acc-compact"),
        include_account_id: true,
        upstream_cookie: Some("cf_clearance=test"),
        incoming_session_id: Some("session-compact"),
        incoming_subagent: Some("compact"),
        fallback_session_id: Some("fallback-session"),
        strip_session_affinity: false,
        has_body: true,
    });

    assert_eq!(
        find_header(&headers, "Authorization").as_deref(),
        Some("Bearer token-compact")
    );
    assert_eq!(
        find_header(&headers, "Content-Type").as_deref(),
        Some("application/json")
    );
    assert_eq!(
        find_header(&headers, "Accept").as_deref(),
        Some("application/json")
    );
    assert_eq!(find_header(&headers, "Version").as_deref(), Some("0.101.0"));
    assert_eq!(
        find_header(&headers, "Session_id").as_deref(),
        Some("session-compact")
    );
    assert_eq!(
        find_header(&headers, "Chatgpt-Account-Id").as_deref(),
        Some("acc-compact")
    );
    assert_eq!(
        find_header(&headers, "Cookie").as_deref(),
        Some("cf_clearance=test")
    );
    assert!(find_header(&headers, "Openai-Beta").is_none());
    assert_eq!(
        find_header(&headers, "Originator").as_deref(),
        Some("codex_cli_rs")
    );
    assert!(find_header(&headers, "User-Agent").is_some());
    assert_eq!(
        find_header(&headers, "x-openai-subagent").as_deref(),
        Some("compact")
    );
    assert!(find_header(&headers, "Conversation_id").is_none());
    assert!(find_header(&headers, "x-codex-turn-state").is_none());
}

#[test]
fn codex_header_profile_uses_dynamic_originator_and_residency_requirement() {
    let _guard = header_runtime_guard();
    let _restore = GatewayHeaderRuntimeRestore::capture();
    crate::set_gateway_originator("codex_cli_rs_e2e").expect("set gateway originator");
    crate::set_gateway_residency_requirement(Some("us"))
        .expect("set gateway residency requirement");

    let headers = build_codex_upstream_headers(CodexUpstreamHeaderInput {
        auth_token: "token-dynamic",
        account_id: None,
        include_account_id: true,
        include_openai_beta: true,
        upstream_cookie: None,
        incoming_session_id: None,
        incoming_client_request_id: None,
        incoming_subagent: None,
        fallback_session_id: None,
        incoming_turn_state: None,
        include_turn_state: true,
        incoming_conversation_id: None,
        fallback_conversation_id: None,
        include_conversation_id: true,
        strip_session_affinity: false,
        is_stream: true,
        has_body: true,
    });

    assert_eq!(
        find_header(&headers, "Originator").as_deref(),
        Some("codex_cli_rs_e2e")
    );
    assert_eq!(
        find_header(&headers, "x-openai-internal-codex-residency").as_deref(),
        Some("us")
    );
    assert!(find_header(&headers, "User-Agent")
        .as_deref()
        .is_some_and(|value| value.contains("codex_cli_rs_e2e/0.101.0")));
}

#[test]
fn codex_header_profile_regenerates_session_on_failover() {
    let headers = build_codex_upstream_headers(CodexUpstreamHeaderInput {
        auth_token: "token-789",
        account_id: None,
        include_account_id: true,
        include_openai_beta: true,
        upstream_cookie: None,
        incoming_session_id: Some("sticky-session"),
        incoming_client_request_id: None,
        incoming_subagent: None,
        fallback_session_id: Some("fallback-session"),
        incoming_turn_state: Some("sticky-turn"),
        include_turn_state: true,
        incoming_conversation_id: Some("sticky-conversation"),
        fallback_conversation_id: Some("fallback-conversation"),
        include_conversation_id: true,
        strip_session_affinity: true,
        is_stream: true,
        has_body: true,
    });

    assert_ne!(
        find_header(&headers, "Session_id").as_deref(),
        Some("sticky-session")
    );
    assert!(find_header(&headers, "x-codex-turn-state").is_none());
    assert!(find_header(&headers, "Conversation_id").is_none());
}

#[test]
fn codex_header_profile_uses_fallback_session_when_incoming_missing() {
    let headers = build_codex_upstream_headers(CodexUpstreamHeaderInput {
        auth_token: "token-fallback",
        account_id: None,
        include_account_id: true,
        include_openai_beta: true,
        upstream_cookie: None,
        incoming_session_id: None,
        incoming_client_request_id: None,
        incoming_subagent: None,
        fallback_session_id: Some("fallback-session"),
        incoming_turn_state: None,
        include_turn_state: true,
        incoming_conversation_id: None,
        fallback_conversation_id: None,
        include_conversation_id: true,
        strip_session_affinity: false,
        is_stream: true,
        has_body: true,
    });

    assert_eq!(
        find_header(&headers, "Session_id").as_deref(),
        Some("fallback-session")
    );
}

#[test]
fn codex_header_profile_uses_fallback_conversation_when_incoming_missing() {
    let headers = build_codex_upstream_headers(CodexUpstreamHeaderInput {
        auth_token: "token-fallback-conv",
        account_id: None,
        include_account_id: true,
        include_openai_beta: true,
        upstream_cookie: None,
        incoming_session_id: None,
        incoming_client_request_id: None,
        incoming_subagent: None,
        fallback_session_id: Some("fallback-session"),
        incoming_turn_state: None,
        include_turn_state: true,
        incoming_conversation_id: None,
        fallback_conversation_id: Some("fallback-conversation"),
        include_conversation_id: true,
        strip_session_affinity: false,
        is_stream: true,
        has_body: true,
    });

    assert_eq!(
        find_header(&headers, "Conversation_id").as_deref(),
        Some("fallback-conversation")
    );
}

#[test]
fn codex_header_profile_skips_account_header_when_disabled() {
    let headers = build_codex_upstream_headers(CodexUpstreamHeaderInput {
        auth_token: "token-no-acc",
        account_id: Some("acc-should-not-send"),
        include_account_id: false,
        include_openai_beta: true,
        upstream_cookie: None,
        incoming_session_id: None,
        incoming_client_request_id: None,
        incoming_subagent: None,
        fallback_session_id: None,
        incoming_turn_state: None,
        include_turn_state: true,
        incoming_conversation_id: None,
        fallback_conversation_id: None,
        include_conversation_id: true,
        strip_session_affinity: false,
        is_stream: true,
        has_body: true,
    });

    assert!(find_header(&headers, "Chatgpt-Account-Id").is_none());
}

#[test]
fn codex_header_profile_can_disable_beta_and_affinity_headers() {
    let headers = build_codex_upstream_headers(CodexUpstreamHeaderInput {
        auth_token: "token-cpa-mode",
        account_id: None,
        include_account_id: true,
        include_openai_beta: false,
        upstream_cookie: None,
        incoming_session_id: Some("sticky-session"),
        incoming_client_request_id: None,
        incoming_subagent: None,
        fallback_session_id: None,
        incoming_turn_state: Some("sticky-turn"),
        include_turn_state: false,
        incoming_conversation_id: Some("sticky-conversation"),
        fallback_conversation_id: None,
        include_conversation_id: false,
        strip_session_affinity: false,
        is_stream: true,
        has_body: true,
    });

    assert!(find_header(&headers, "Openai-Beta").is_none());
    assert!(find_header(&headers, "x-codex-turn-state").is_none());
    assert!(find_header(&headers, "Conversation_id").is_none());
    assert_eq!(
        find_header(&headers, "Session_id").as_deref(),
        Some("sticky-session")
    );
}
