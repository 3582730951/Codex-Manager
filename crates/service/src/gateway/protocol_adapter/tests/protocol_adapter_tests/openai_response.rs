#[allow(unused_imports)]
use super::{
    adapt_upstream_response, convert_openai_chat_stream_chunk,
    convert_openai_completions_stream_chunk, ResponseAdapter,
};

#[test]
fn openai_chat_response_is_converted_from_responses_json() {
    let upstream = br#"{
        "id":"resp_1",
        "object":"response",
        "created":1700000000,
        "model":"gpt-5.3-codex",
        "output":[{"type":"message","content":[{"type":"output_text","text":"hello world"}]}],
        "usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12}
    }"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("application/json"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value.get("object").and_then(serde_json::Value::as_str),
        Some("chat.completion")
    );
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str),
        Some("hello world")
    );
}

#[test]
fn openai_chat_response_is_converted_from_output_text_item() {
    let upstream = br#"{
        "id":"resp_1",
        "object":"response",
        "created":1700000000,
        "model":"gpt-5.3-codex",
        "output":[{"type":"output_text","text":"plain output item text"}],
        "usage":{"input_tokens":10,"output_tokens":4,"total_tokens":14}
    }"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("application/json"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str),
        Some("plain output item text")
    );
}

#[test]
fn openai_chat_response_is_converted_from_openclaw_tool_call_json() {
    let upstream = br#"{
        "id":"resp_openclaw_tool_1",
        "object":"response",
        "created_at":1700000011,
        "status":"incomplete",
        "model":"openclaw",
        "output":[{
            "type":"function_call",
            "id":"call_item_1",
            "call_id":"call_weather_1",
            "name":"get_weather",
            "arguments":"{\"city\":\"Shanghai\"}"
        }],
        "usage":{"input_tokens":12,"output_tokens":3,"total_tokens":15}
    }"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("application/json"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value.get("created").and_then(serde_json::Value::as_i64),
        Some(1700000011)
    );
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(serde_json::Value::as_str),
        Some("tool_calls")
    );
    assert!(value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .is_some_and(serde_json::Value::is_null));
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("tool_calls"))
            .and_then(|tool_calls| tool_calls.get(0))
            .and_then(|tool_call| tool_call.get("id"))
            .and_then(serde_json::Value::as_str),
        Some("call_weather_1")
    );
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("tool_calls"))
            .and_then(|tool_calls| tool_calls.get(0))
            .and_then(|tool_call| tool_call.get("function"))
            .and_then(|function| function.get("name"))
            .and_then(serde_json::Value::as_str),
        Some("get_weather")
    );
}

#[test]
fn openai_chat_response_is_converted_from_custom_tool_call_json() {
    let upstream = br#"{
        "id":"resp_custom_tool_1",
        "object":"response",
        "created":1700000012,
        "model":"gpt-5.3-codex",
        "output":[{"type":"custom_tool_call","call_id":"call_exec_1","name":"exec","input":"{\"cmd\":\"pwd\"}"}],
        "usage":{"input_tokens":12,"output_tokens":3,"total_tokens":15}
    }"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("application/json"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value["choices"][0]["message"]["tool_calls"][0]["id"],
        "call_exec_1"
    );
    assert_eq!(
        value["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
        "exec"
    );
    assert_eq!(
        value["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        "{\"cmd\":\"pwd\"}"
    );
    assert_eq!(value["choices"][0]["finish_reason"], "tool_calls");
}

#[test]
fn openai_chat_response_is_converted_from_web_search_call_json() {
    let upstream = br#"{
        "id":"resp_web_search_1",
        "object":"response",
        "created":1700000013,
        "model":"gpt-5.3-codex",
        "output":[{"type":"web_search_call","id":"ws_1","status":"completed","action":{"type":"search","query":"weather seattle"}}],
        "usage":{"input_tokens":9,"output_tokens":2,"total_tokens":11}
    }"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("application/json"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value["choices"][0]["message"]["content"],
        "[web_search_call] status=completed query=weather seattle"
    );
}

#[test]
fn openai_chat_response_is_converted_from_image_generation_call_json() {
    let upstream = br#"{
        "id":"resp_image_1",
        "object":"response",
        "created":1700000014,
        "model":"gpt-5.3-codex",
        "output":[{"type":"image_generation_call","id":"ig_1","status":"completed","revised_prompt":"A small blue square","result":"Zm9v"}],
        "usage":{"input_tokens":9,"output_tokens":2,"total_tokens":11}
    }"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("application/json"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value["choices"][0]["message"]["content"],
        "[image_generation_call] status=completed prompt=A small blue square result_bytes=4"
    );
}

#[test]
fn openai_chat_response_is_converted_from_local_shell_call_json() {
    let upstream = br#"{
        "id":"resp_shell_1",
        "object":"response",
        "created":1700000015,
        "model":"gpt-5.3-codex",
        "output":[{"type":"local_shell_call","call_id":"shell_1","status":"completed","action":{"type":"exec","command":["/bin/echo","hello"],"working_directory":"/tmp"}}],
        "usage":{"input_tokens":9,"output_tokens":2,"total_tokens":11}
    }"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("application/json"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value["choices"][0]["message"]["content"],
        "[local_shell_call] status=completed command=/bin/echo hello cwd=/tmp"
    );
}

#[test]
fn openai_chat_response_is_converted_from_custom_tool_call_output_json() {
    let upstream = br#"{
        "id":"resp_custom_tool_output_1",
        "object":"response",
        "created":1700000016,
        "model":"gpt-5.3-codex",
        "output":[{"type":"custom_tool_call_output","call_id":"call_exec_1","output":"command finished"}],
        "usage":{"input_tokens":9,"output_tokens":2,"total_tokens":11}
    }"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("application/json"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value["choices"][0]["message"]["content"],
        "command finished"
    );
}

#[test]
fn openai_chat_stream_response_is_collapsed_to_chat_completion_json() {
    let upstream = br#"data: {"type":"response.output_text.delta","response_id":"resp_1","created":1700000001,"model":"gpt-5.3-codex","delta":"hello "}

data: {"type":"response.output_text.delta","response_id":"resp_1","created":1700000001,"model":"gpt-5.3-codex","delta":"world"}

data: {"type":"response.completed","response":{"id":"resp_1","created":1700000001,"model":"gpt-5.3-codex","usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsSse,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str),
        Some("hello world")
    );
}

#[test]
fn openai_chat_stream_collapse_avoids_done_and_item_text_duplication() {
    let upstream = br#"data: {"type":"response.output_text.delta","response_id":"resp_dup_1","created":1700000010,"model":"gpt-5.3-codex","delta":"hello "}

data: {"type":"response.output_text.delta","response_id":"resp_dup_1","created":1700000010,"model":"gpt-5.3-codex","delta":"world"}

data: {"type":"response.output_text.done","response_id":"resp_dup_1","created":1700000010,"model":"gpt-5.3-codex","text":"hello world"}

data: {"type":"response.content_part.done","response_id":"resp_dup_1","created":1700000010,"model":"gpt-5.3-codex","part":{"type":"output_text","text":"hello world"}}

data: {"type":"response.output_item.done","response_id":"resp_dup_1","created":1700000010,"model":"gpt-5.3-codex","item":{"type":"message","content":[{"type":"output_text","text":"hello world"}]}}

data: {"type":"response.completed","response":{"id":"resp_dup_1","created":1700000010,"model":"gpt-5.3-codex","usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str),
        Some("hello world")
    );
}

#[test]
fn openai_chat_stream_response_accepts_output_item_done_text() {
    let upstream = br#"data: {"type":"response.output_item.done","response_id":"resp_2","created":1700000002,"model":"gpt-5.3-codex","item":{"type":"message","content":[{"type":"output_text","text":"from output item"}]}}

data: {"type":"response.completed","response":{"id":"resp_2","created":1700000002,"model":"gpt-5.3-codex","usage":{"input_tokens":8,"output_tokens":3,"total_tokens":11}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str),
        Some("from output item")
    );
}

#[test]
fn openai_chat_stream_response_accepts_output_item_added_text() {
    let upstream = br#"data: {"type":"response.output_item.added","response_id":"resp_2b","created":1700000002,"model":"gpt-5.3-codex","item":{"type":"message","content":[{"type":"output_text","text":"from output item added"}]}}

data: {"type":"response.completed","response":{"id":"resp_2b","created":1700000002,"model":"gpt-5.3-codex","usage":{"input_tokens":8,"output_tokens":3,"total_tokens":11}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str),
        Some("from output item added")
    );
}

#[test]
fn openai_chat_stream_response_completed_only_preserves_tool_calls() {
    let upstream = br#"data: {"type":"response.completed","response":{"id":"resp_tool_only","created":1700000005,"model":"gpt-5.3-codex","output":[{"type":"function_call","call_id":"call_tool_only","name":"read_file","arguments":"{\"path\":\"README.md\"}"}],"usage":{"input_tokens":7,"output_tokens":3,"total_tokens":10}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content")),
        Some(&serde_json::Value::Null)
    );
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("tool_calls"))
            .and_then(|tool_calls| tool_calls.get(0))
            .and_then(|tool_call| tool_call.get("id"))
            .and_then(serde_json::Value::as_str),
        Some("call_tool_only")
    );
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("tool_calls"))
            .and_then(|tool_calls| tool_calls.get(0))
            .and_then(|tool_call| tool_call.get("function"))
            .and_then(|function| function.get("name"))
            .and_then(serde_json::Value::as_str),
        Some("read_file")
    );
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("tool_calls"))
            .and_then(|tool_calls| tool_calls.get(0))
            .and_then(|tool_call| tool_call.get("function"))
            .and_then(|function| function.get("arguments"))
            .and_then(serde_json::Value::as_str),
        Some("{\"path\":\"README.md\"}")
    );
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(serde_json::Value::as_str),
        Some("tool_calls")
    );
}

#[test]
fn openai_chat_stream_response_delta_only_preserves_tool_calls() {
    let upstream = br#"data: {"type":"response.output_item.added","response_id":"resp_tool_delta","created":1700000006,"model":"gpt-5.3-codex","output_index":0,"item":{"type":"function_call","call_id":"call_tool_delta","name":"read_file"}}

data: {"type":"response.function_call_arguments.delta","response_id":"resp_tool_delta","created":1700000006,"model":"gpt-5.3-codex","output_index":0,"delta":"{\"path\":\"REA"}

data: {"type":"response.function_call_arguments.delta","response_id":"resp_tool_delta","created":1700000006,"model":"gpt-5.3-codex","output_index":0,"delta":"DME.md\"}"}

data: {"type":"response.completed","response":{"id":"resp_tool_delta","created":1700000006,"model":"gpt-5.3-codex","usage":{"input_tokens":8,"output_tokens":2,"total_tokens":10}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("tool_calls"))
            .and_then(|tool_calls| tool_calls.get(0))
            .and_then(|tool_call| tool_call.get("id"))
            .and_then(serde_json::Value::as_str),
        Some("call_tool_delta")
    );
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("tool_calls"))
            .and_then(|tool_calls| tool_calls.get(0))
            .and_then(|tool_call| tool_call.get("function"))
            .and_then(|function| function.get("arguments"))
            .and_then(serde_json::Value::as_str),
        Some("{\"path\":\"README.md\"}")
    );
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(serde_json::Value::as_str),
        Some("tool_calls")
    );
}

#[test]
fn openai_chat_stream_response_delta_only_preserves_custom_tool_calls() {
    let upstream = br#"data: {"type":"response.output_item.added","response_id":"resp_custom_tool_delta","created":1700000007,"model":"gpt-5.3-codex","output_index":0,"item":{"type":"custom_tool_call","call_id":"call_exec_delta","name":"exec"}}

data: {"type":"response.output_item.done","response_id":"resp_custom_tool_delta","created":1700000007,"model":"gpt-5.3-codex","output_index":0,"item":{"type":"custom_tool_call","call_id":"call_exec_delta","name":"exec","input":"{\"cmd\":\"pwd\"}"}}

data: {"type":"response.completed","response":{"id":"resp_custom_tool_delta","created":1700000007,"model":"gpt-5.3-codex","usage":{"input_tokens":8,"output_tokens":2,"total_tokens":10}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value["choices"][0]["message"]["tool_calls"][0]["id"],
        "call_exec_delta"
    );
    assert_eq!(
        value["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
        "exec"
    );
    assert_eq!(
        value["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        "{\"cmd\":\"pwd\"}"
    );
    assert_eq!(value["choices"][0]["finish_reason"], "tool_calls");
}

#[test]
fn openai_chat_stream_response_delta_only_preserves_custom_tool_calls_from_incremental_input() {
    let upstream = br#"data: {"type":"response.output_item.added","response_id":"resp_custom_tool_incremental","created":1700000007,"model":"gpt-5.3-codex","output_index":0,"item":{"type":"custom_tool_call","call_id":"call_js_delta","name":"js_repl"}}

data: {"type":"response.custom_tool_call_input.delta","response_id":"resp_custom_tool_incremental","created":1700000007,"model":"gpt-5.3-codex","output_index":0,"call_id":"call_js_delta","delta":"console.log(\"hel"}

data: {"type":"response.custom_tool_call_input.done","response_id":"resp_custom_tool_incremental","created":1700000007,"model":"gpt-5.3-codex","output_index":0,"call_id":"call_js_delta","input":"console.log(\"hello\")"}

data: {"type":"response.completed","response":{"id":"resp_custom_tool_incremental","created":1700000007,"model":"gpt-5.3-codex","usage":{"input_tokens":8,"output_tokens":2,"total_tokens":10}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value["choices"][0]["message"]["tool_calls"][0]["id"],
        "call_js_delta"
    );
    assert_eq!(
        value["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
        "js_repl"
    );
    assert_eq!(
        value["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        "console.log(\"hello\")"
    );
    assert_eq!(value["choices"][0]["finish_reason"], "tool_calls");
}

#[test]
fn openai_chat_stream_response_outputs_web_search_summary_text() {
    let upstream = br#"data: {"type":"response.output_item.done","response_id":"resp_web_search_stream","created":1700000008,"model":"gpt-5.3-codex","output_index":0,"item":{"type":"web_search_call","id":"ws_1","status":"completed","action":{"type":"search","query":"weather seattle"}}}

data: {"type":"response.completed","response":{"id":"resp_web_search_stream","created":1700000008,"model":"gpt-5.3-codex","usage":{"input_tokens":8,"output_tokens":2,"total_tokens":10}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value["choices"][0]["message"]["content"],
        "[web_search_call] status=completed query=weather seattle"
    );
}

#[test]
fn openai_chat_stream_chunk_maps_function_call_argument_delta() {
    let value = serde_json::json!({
        "type": "response.function_call_arguments.delta",
        "response_id": "resp_call_1",
        "created": 1700000100,
        "model": "gpt-5.3-codex",
        "output_index": 0,
        "delta": "{\"x\":1}"
    });
    let mapped =
        convert_openai_chat_stream_chunk(&value).expect("map function_call_arguments.delta");
    assert_eq!(
        mapped.get("object").and_then(serde_json::Value::as_str),
        Some("chat.completion.chunk")
    );
    assert_eq!(
        mapped
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("tool_calls"))
            .and_then(|tool_calls| tool_calls.get(0))
            .and_then(|tool_call| tool_call.get("function"))
            .and_then(|function| function.get("arguments"))
            .and_then(serde_json::Value::as_str),
        Some("{\"x\":1}")
    );
}

#[test]
fn openai_chat_stream_chunk_maps_custom_tool_input_delta() {
    let value = serde_json::json!({
        "type": "response.custom_tool_call_input.delta",
        "response_id": "resp_call_custom_1",
        "created": 1700000100,
        "model": "gpt-5.3-codex",
        "output_index": 0,
        "call_id": "call_custom_1",
        "delta": "console.log(1)"
    });
    let mapped =
        convert_openai_chat_stream_chunk(&value).expect("map custom_tool_call_input.delta");
    assert_eq!(
        mapped
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("tool_calls"))
            .and_then(|tool_calls| tool_calls.get(0))
            .and_then(|tool_call| tool_call.get("id"))
            .and_then(serde_json::Value::as_str),
        Some("call_custom_1")
    );
    assert_eq!(
        mapped
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("tool_calls"))
            .and_then(|tool_calls| tool_calls.get(0))
            .and_then(|tool_call| tool_call.get("function"))
            .and_then(|function| function.get("arguments"))
            .and_then(serde_json::Value::as_str),
        Some("console.log(1)")
    );
}

#[test]
fn openai_chat_stream_chunk_prefers_call_id_over_item_id_for_function_call_delta() {
    let value = serde_json::json!({
        "type": "response.function_call_arguments.delta",
        "response_id": "resp_call_2",
        "created": 1700000100,
        "model": "gpt-5.3-codex",
        "output_index": 0,
        "call_id": "call_real",
        "item_id": "item_shadow",
        "delta": "{\"x\":1}"
    });
    let mapped = convert_openai_chat_stream_chunk(&value).expect("map function_call_arguments");
    assert_eq!(
        mapped
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("tool_calls"))
            .and_then(|tool_calls| tool_calls.get(0))
            .and_then(|tool_call| tool_call.get("id"))
            .and_then(serde_json::Value::as_str),
        Some("call_real")
    );
}

#[test]
fn openai_chat_stream_chunk_fallback_maps_unknown_text_event() {
    let value = serde_json::json!({
        "type": "response.output_markdown.delta",
        "response_id": "resp_txt_1",
        "created": 1700000101,
        "model": "gpt-5.3-codex",
        "delta": "fallback text"
    });
    let mapped = convert_openai_chat_stream_chunk(&value).expect("map unknown text event");
    assert_eq!(
        mapped
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("content"))
            .and_then(serde_json::Value::as_str),
        Some("fallback text")
    );
}

#[test]
fn openai_completions_stream_chunk_fallback_maps_unknown_text_event() {
    let value = serde_json::json!({
        "type": "response.output_markdown.delta",
        "response_id": "resp_txt_2",
        "created": 1700000102,
        "model": "gpt-5.3-codex",
        "delta": "completion fallback"
    });
    let mapped = convert_openai_completions_stream_chunk(&value).expect("map unknown text event");
    assert_eq!(
        mapped
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("text"))
            .and_then(serde_json::Value::as_str),
        Some("completion fallback")
    );
}

#[test]
fn openai_chat_stream_response_completed_only_still_outputs_text() {
    let upstream = br#"data: {"type":"response.completed","response":{"id":"resp_3","created":1700000003,"model":"gpt-5.3-codex","output":[{"type":"message","content":[{"type":"output_text","text":"completed only text"}]}],"usage":{"input_tokens":8,"output_tokens":3,"total_tokens":11}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str),
        Some("completed only text")
    );
}

#[test]
fn openai_chat_stream_event_only_completed_still_outputs_text() {
    let upstream = br#"event: response.completed
data: {"response":{"id":"resp_3_evt","created":1700000003,"model":"gpt-5.3-codex","output":[{"type":"message","content":[{"type":"output_text","text":"event completed only text"}]}],"usage":{"input_tokens":8,"output_tokens":3,"total_tokens":11}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str),
        Some("event completed only text")
    );
}

#[test]
fn openai_chat_stream_response_done_only_still_outputs_text() {
    let upstream = br#"data: {"type":"response.done","response":{"id":"resp_3_done","created":1700000003,"model":"gpt-5.3-codex","output":[{"type":"message","content":[{"type":"output_text","text":"done only text"}]}],"usage":{"input_tokens":8,"output_tokens":3,"total_tokens":11}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAIChatCompletionsJson,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(serde_json::Value::as_str),
        Some("done only text")
    );
}

#[test]
fn openai_completions_response_is_converted_from_responses_json() {
    let upstream = br#"{
        "id":"resp_1",
        "object":"response",
        "created":1700000000,
        "model":"gpt-5.3-codex",
        "output":[{"type":"message","content":[{"type":"output_text","text":"hello world"}]}],
        "usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12}
    }"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAICompletionsJson,
        Some("application/json"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value.get("object").and_then(serde_json::Value::as_str),
        Some("text_completion")
    );
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("text"))
            .and_then(serde_json::Value::as_str),
        Some("hello world")
    );
}

#[test]
fn openai_completions_stream_completed_only_still_outputs_text() {
    let upstream = br#"data: {"type":"response.completed","response":{"id":"resp_4","created":1700000004,"model":"gpt-5.3-codex","output":[{"type":"message","content":[{"type":"output_text","text":"completed only completion text"}]}],"usage":{"input_tokens":9,"output_tokens":4,"total_tokens":13}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAICompletionsJson,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("text"))
            .and_then(serde_json::Value::as_str),
        Some("completed only completion text")
    );
}

#[test]
fn openai_completions_stream_event_only_done_still_outputs_text() {
    let upstream = br#"event: response.done
data: {"response":{"id":"resp_4_done_evt","created":1700000004,"model":"gpt-5.3-codex","output":[{"type":"message","content":[{"type":"output_text","text":"event done only completion text"}]}],"usage":{"input_tokens":9,"output_tokens":4,"total_tokens":13}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAICompletionsJson,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("text"))
            .and_then(serde_json::Value::as_str),
        Some("event done only completion text")
    );
}

#[test]
fn openai_completions_stream_done_only_still_outputs_text() {
    let upstream = br#"data: {"type":"response.done","response":{"id":"resp_4_done","created":1700000004,"model":"gpt-5.3-codex","output":[{"type":"message","content":[{"type":"output_text","text":"done only completion text"}]}],"usage":{"input_tokens":9,"output_tokens":4,"total_tokens":13}}}

data: [DONE]

"#;
    let (body, content_type) = adapt_upstream_response(
        ResponseAdapter::OpenAICompletionsJson,
        Some("text/event-stream"),
        upstream,
    )
    .expect("convert response");
    let value: serde_json::Value = serde_json::from_slice(&body).expect("parse converted body");
    assert_eq!(content_type, "application/json");
    assert_eq!(
        value
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("text"))
            .and_then(serde_json::Value::as_str),
        Some("done only completion text")
    );
}
