use super::*;
use codexmanager_core::storage::Account;
use reqwest::header::HeaderValue;

fn build_account(account_id: &str) -> Account {
    Account {
        id: account_id.to_string(),
        label: account_id.to_string(),
        issuer: "issuer".to_string(),
        chatgpt_account_id: Some(format!("chatgpt-{account_id}")),
        workspace_id: Some(format!("workspace-{account_id}")),
        group_name: None,
        sort: 0,
        status: "active".to_string(),
        created_at: 1,
        updated_at: 1,
    }
}

#[test]
fn status_404_with_more_candidates_triggers_failover() {
    let storage = Storage::open_in_memory().expect("open");
    storage.init().expect("init");
    let decision = decide_upstream_outcome(
        &storage,
        &build_account("acc-404"),
        reqwest::StatusCode::NOT_FOUND,
        None,
        "https://chatgpt.com/backend-api/codex/chat/completions",
        true,
        |_, _, _| {},
    );
    assert!(matches!(decision, UpstreamOutcomeDecision::Failover));
}

#[test]
fn status_404_on_last_candidate_keeps_upstream_response() {
    let storage = Storage::open_in_memory().expect("open");
    storage.init().expect("init");
    let decision = decide_upstream_outcome(
        &storage,
        &build_account("acc-404"),
        reqwest::StatusCode::NOT_FOUND,
        None,
        "https://chatgpt.com/backend-api/codex/chat/completions",
        false,
        |_, _, _| {},
    );
    assert!(matches!(decision, UpstreamOutcomeDecision::RespondUpstream));
}

#[test]
fn status_429_with_more_candidates_triggers_failover() {
    let storage = Storage::open_in_memory().expect("open");
    storage.init().expect("init");
    let decision = decide_upstream_outcome(
        &storage,
        &build_account("acc-429"),
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        None,
        "https://api.openai.com/v1/responses",
        true,
        |_, _, _| {},
    );
    assert!(matches!(decision, UpstreamOutcomeDecision::Failover));
}

#[test]
fn status_429_on_last_candidate_keeps_upstream_response() {
    let storage = Storage::open_in_memory().expect("open");
    storage.init().expect("init");
    let decision = decide_upstream_outcome(
        &storage,
        &build_account("acc-429"),
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        None,
        "https://api.openai.com/v1/responses",
        false,
        |_, _, _| {},
    );
    assert!(matches!(decision, UpstreamOutcomeDecision::RespondUpstream));
}

#[test]
fn challenge_with_more_candidates_triggers_failover() {
    let storage = Storage::open_in_memory().expect("open");
    storage.init().expect("init");
    let content_type = HeaderValue::from_static("text/html; charset=utf-8");
    let decision = decide_upstream_outcome(
        &storage,
        &build_account("acc-challenge"),
        reqwest::StatusCode::FORBIDDEN,
        Some(&content_type),
        "https://chatgpt.com/backend-api/codex/responses",
        true,
        |_, _, _| {},
    );
    assert!(matches!(decision, UpstreamOutcomeDecision::Failover));
}

#[test]
fn challenge_on_last_candidate_keeps_upstream_response() {
    let storage = Storage::open_in_memory().expect("open");
    storage.init().expect("init");
    let content_type = HeaderValue::from_static("text/html; charset=utf-8");
    let decision = decide_upstream_outcome(
        &storage,
        &build_account("acc-challenge"),
        reqwest::StatusCode::FORBIDDEN,
        Some(&content_type),
        "https://chatgpt.com/backend-api/codex/responses",
        false,
        |_, _, _| {},
    );
    assert!(matches!(decision, UpstreamOutcomeDecision::RespondUpstream));
}

#[test]
fn status_500_with_more_candidates_triggers_failover() {
    let storage = Storage::open_in_memory().expect("open");
    storage.init().expect("init");
    let decision = decide_upstream_outcome(
        &storage,
        &build_account("acc-500"),
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        None,
        "https://chatgpt.com/backend-api/codex/responses",
        true,
        |_, _, _| {},
    );
    assert!(matches!(decision, UpstreamOutcomeDecision::Failover));
}

#[test]
fn status_500_on_last_candidate_keeps_upstream_response() {
    let storage = Storage::open_in_memory().expect("open");
    storage.init().expect("init");
    let decision = decide_upstream_outcome(
        &storage,
        &build_account("acc-500"),
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        None,
        "https://chatgpt.com/backend-api/codex/responses",
        false,
        |_, _, _| {},
    );
    assert!(matches!(decision, UpstreamOutcomeDecision::RespondUpstream));
}
