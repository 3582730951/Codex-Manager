use super::*;

#[test]
fn estimate_input_tokens_uses_messages_and_system_text() {
    let body = br#"{
        "model":"gpt-5.3-codex",
        "system":"abcdabcd",
        "messages":[
            {"role":"user","content":"abcd"},
            {"role":"assistant","content":[{"type":"text","text":"abcdabcd"}]}
        ]
    }"#;
    let count = estimate_input_tokens_from_anthropic_messages(body).expect("estimate failed");
    assert_eq!(count, 5);
}

#[test]
fn estimate_input_tokens_rejects_invalid_json() {
    let err = estimate_input_tokens_from_anthropic_messages(br#"{"messages":["#)
        .expect_err("should reject invalid json");
    assert_eq!(err, "invalid claude request json");
}

#[test]
fn estimate_input_tokens_rejects_non_object_payload() {
    let err = estimate_input_tokens_from_anthropic_messages(br#"["bad"]"#)
        .expect_err("should reject non-object payload");
    assert_eq!(err, "claude request body must be an object");
}

#[test]
fn should_handle_local_count_tokens_only_matches_anthropic_post_endpoint() {
    assert!(should_handle_local_count_tokens(
        crate::apikey_profile::PROTOCOL_ANTHROPIC_NATIVE,
        "POST",
        "/v1/messages/count_tokens"
    ));
    assert!(!should_handle_local_count_tokens(
        crate::apikey_profile::PROTOCOL_OPENAI_COMPAT,
        "POST",
        "/v1/messages/count_tokens"
    ));
    assert!(!should_handle_local_count_tokens(
        crate::apikey_profile::PROTOCOL_ANTHROPIC_NATIVE,
        "GET",
        "/v1/messages/count_tokens"
    ));
}
