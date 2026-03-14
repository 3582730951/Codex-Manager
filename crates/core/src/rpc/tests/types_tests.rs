use super::{AccountListParams, AccountListResult, AccountSummary, RequestLogSummary};

#[test]
fn account_summary_serialization_matches_compact_contract() {
    let summary = AccountSummary {
        id: "acc-1".to_string(),
        label: "主账号".to_string(),
        group_name: Some("TEAM".to_string()),
        sort: 10,
        status: "active".to_string(),
    };

    let value = serde_json::to_value(summary).expect("serialize account summary");
    let obj = value.as_object().expect("account summary object");

    for key in ["id", "label", "groupName", "sort", "status"] {
        assert!(obj.contains_key(key), "missing key: {key}");
    }

    for key in ["workspaceId", "workspaceName", "note", "tags", "updatedAt"] {
        assert!(!obj.contains_key(key), "unexpected key: {key}");
    }
}

#[test]
fn account_list_params_default_to_first_page_with_five_items() {
    let params: AccountListParams =
        serde_json::from_value(serde_json::json!({})).expect("deserialize params");
    let normalized = params.normalized();

    assert_eq!(normalized.page, 1);
    assert_eq!(normalized.page_size, 5);
}

#[test]
fn account_list_result_serialization_includes_pagination_fields() {
    let result = AccountListResult {
        items: vec![AccountSummary {
            id: "acc-1".to_string(),
            label: "主账号".to_string(),
            group_name: Some("TEAM".to_string()),
            sort: 10,
            status: "active".to_string(),
        }],
        total: 9,
        page: 2,
        page_size: 3,
    };

    let value = serde_json::to_value(result).expect("serialize account list result");
    let obj = value.as_object().expect("account list result object");
    for key in ["items", "total", "page", "pageSize"] {
        assert!(obj.contains_key(key), "missing key: {key}");
    }
}

#[test]
fn request_log_summary_serialization_includes_trace_route_fields() {
    let summary = RequestLogSummary {
        trace_id: Some("trc_1".to_string()),
        key_id: Some("gk_1".to_string()),
        account_id: Some("acc_1".to_string()),
        request_path: "/v1/responses".to_string(),
        original_path: Some("/v1/chat/completions".to_string()),
        adapted_path: Some("/v1/responses".to_string()),
        method: "POST".to_string(),
        model: Some("gpt-5.3-codex".to_string()),
        reasoning_effort: Some("high".to_string()),
        response_adapter: Some("OpenAIChatCompletionsJson".to_string()),
        upstream_url: Some("https://api.openai.com/v1".to_string()),
        status_code: Some(502),
        duration_ms: Some(1450),
        input_tokens: Some(10),
        cached_input_tokens: Some(0),
        output_tokens: Some(3),
        total_tokens: Some(13),
        reasoning_output_tokens: Some(1),
        estimated_cost_usd: Some(0.12),
        error: Some("internal_error".to_string()),
        created_at: 1,
    };

    let value = serde_json::to_value(summary).expect("serialize request log summary");
    let obj = value.as_object().expect("request log summary object");
    for key in [
        "traceId",
        "originalPath",
        "adaptedPath",
        "responseAdapter",
        "requestPath",
        "upstreamUrl",
        "durationMs",
    ] {
        assert!(obj.contains_key(key), "missing key: {key}");
    }
}
