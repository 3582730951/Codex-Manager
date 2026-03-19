use super::*;
use reqwest::header::HeaderValue;

#[test]
fn status_404_with_more_candidates_triggers_failover() {
    let storage = Storage::open_in_memory().expect("open");
    storage.init().expect("init");
    let decision = decide_upstream_outcome(
        &storage,
        "acc-404",
        "/backend-api/codex/chat/completions",
        None,
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
        "acc-404",
        "/backend-api/codex/chat/completions",
        None,
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
        "acc-429",
        "/v1/responses",
        Some("gpt-5.4"),
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
        "acc-429",
        "/v1/responses",
        Some("gpt-5.4"),
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
        "acc-challenge",
        "/backend-api/codex/responses",
        Some("gpt-5.4"),
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
        "acc-challenge",
        "/backend-api/codex/responses",
        Some("gpt-5.4"),
        reqwest::StatusCode::FORBIDDEN,
        Some(&content_type),
        "https://chatgpt.com/backend-api/codex/responses",
        false,
        |_, _, _| {},
    );
    assert!(matches!(decision, UpstreamOutcomeDecision::RespondUpstream));
}

#[test]
fn likely_model_ineligible_400_with_more_candidates_triggers_failover_and_feedback() {
    crate::gateway::clear_request_feedback_runtime_state_for_tests();
    let storage = Storage::open_in_memory().expect("open");
    storage.init().expect("init");
    let decision = decide_upstream_outcome(
        &storage,
        "acc-model",
        "/v1/responses",
        Some("gpt-5.4"),
        reqwest::StatusCode::BAD_REQUEST,
        None,
        "https://api.openai.com/v1/responses",
        true,
        |_, _, _| {},
    );
    assert!(matches!(decision, UpstreamOutcomeDecision::Failover));
    assert_eq!(
        crate::gateway::request_feedback_for("acc-model", Some("gpt-5.4")),
        Some(crate::gateway::AccountRequestFeedback::ModelIneligible)
    );
}

#[test]
fn likely_model_ineligible_422_with_more_candidates_triggers_failover_and_feedback() {
    crate::gateway::clear_request_feedback_runtime_state_for_tests();
    let storage = Storage::open_in_memory().expect("open");
    storage.init().expect("init");
    let decision = decide_upstream_outcome(
        &storage,
        "acc-model-422",
        "/v1/responses",
        Some("gpt-5.4"),
        reqwest::StatusCode::UNPROCESSABLE_ENTITY,
        None,
        "https://api.openai.com/v1/responses",
        true,
        |_, _, _| {},
    );
    assert!(matches!(decision, UpstreamOutcomeDecision::Failover));
    assert_eq!(
        crate::gateway::request_feedback_for("acc-model-422", Some("gpt-5.4")),
        Some(crate::gateway::AccountRequestFeedback::ModelIneligible)
    );
}

#[test]
fn likely_quota_rejected_403_with_more_candidates_triggers_failover_and_feedback() {
    crate::gateway::clear_request_feedback_runtime_state_for_tests();
    let storage = Storage::open_in_memory().expect("open");
    storage.init().expect("init");
    let decision = decide_upstream_outcome(
        &storage,
        "acc-quota",
        "/v1/chat/completions",
        Some("gpt-5.4"),
        reqwest::StatusCode::FORBIDDEN,
        None,
        "https://api.openai.com/v1/chat/completions",
        true,
        |_, _, _| {},
    );
    assert!(matches!(decision, UpstreamOutcomeDecision::Failover));
    assert_eq!(
        crate::gateway::request_feedback_for("acc-quota", Some("gpt-5.4")),
        Some(crate::gateway::AccountRequestFeedback::QuotaRejected)
    );
}
