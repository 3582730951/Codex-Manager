use serde_json::{json, Value};

use super::super::aggregate::collect_response_output_text;

#[derive(Debug, Clone, Default)]
pub(in super::super) struct OpenAIStreamMeta {
    pub(in super::super) response_id: Option<String>,
    pub(in super::super) model: Option<String>,
    pub(in super::super) created: Option<i64>,
}

pub(in super::super) fn update_openai_stream_meta(meta: &mut OpenAIStreamMeta, value: &Value) {
    let response = value.get("response");

    if meta.response_id.is_none() {
        meta.response_id = value
            .get("response_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .or_else(|| {
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
            })
            .or_else(|| {
                response
                    .and_then(|response| response.get("id"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
            });
    }

    if meta.model.is_none() {
        meta.model = value
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_string)
            .or_else(|| {
                response
                    .and_then(|response| response.get("model"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|model| !model.is_empty())
                    .map(str::to_string)
            });
    }

    if meta.created.is_none() {
        meta.created = value
            .get("created")
            .and_then(Value::as_i64)
            .or_else(|| value.get("created_at").and_then(Value::as_i64))
            .or_else(|| {
                response
                    .and_then(|response| response.get("created"))
                    .and_then(Value::as_i64)
            })
            .or_else(|| {
                response
                    .and_then(|response| response.get("created_at"))
                    .and_then(Value::as_i64)
            });
    }
}

pub(in super::super) fn apply_openai_stream_meta_defaults(
    mapped: &mut Value,
    meta: &OpenAIStreamMeta,
) {
    let Some(mapped_obj) = mapped.as_object_mut() else {
        return;
    };
    if let Some(id) = meta.response_id.as_deref() {
        let needs_id = mapped_obj
            .get("id")
            .and_then(Value::as_str)
            .is_none_or(|current| current.is_empty());
        if needs_id {
            mapped_obj.insert("id".to_string(), Value::String(id.to_string()));
        }
    }
    if let Some(model) = meta.model.as_deref() {
        let needs_model = mapped_obj
            .get("model")
            .and_then(Value::as_str)
            .is_none_or(|current| current.is_empty());
        if needs_model {
            mapped_obj.insert("model".to_string(), Value::String(model.to_string()));
        }
    }
    if let Some(created) = meta.created {
        let needs_created = mapped_obj
            .get("created")
            .and_then(Value::as_i64)
            .is_none_or(|current| current == 0);
        if needs_created {
            mapped_obj.insert("created".to_string(), Value::Number(created.into()));
        }
    }
}

pub(in super::super) fn extract_openai_completed_output_text(value: &Value) -> Option<String> {
    let response = value.get("response").unwrap_or(value);
    let mut output_text = String::new();
    collect_response_output_text(response, &mut output_text);
    let trimmed = output_text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(in super::super) fn map_chunk_has_chat_text(mapped: &Value) -> bool {
    mapped
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| {
            choices.iter().any(|choice| {
                choice
                    .get("delta")
                    .and_then(Value::as_object)
                    .and_then(|delta| delta.get("content"))
                    .and_then(Value::as_str)
                    .is_some_and(|content| !content.is_empty())
            })
        })
}

pub(in super::super) fn map_chunk_has_completion_text(mapped: &Value) -> bool {
    mapped
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| {
            choices.iter().any(|choice| {
                choice
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
            })
        })
}

fn is_tool_related_output_item(value: &Value) -> bool {
    value
        .get("item")
        .or_else(|| value.get("output_item"))
        .and_then(|item| item.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|item_type| {
            matches!(
                item_type,
                "function_call"
                    | "custom_tool_call"
                    | "function_call_output"
                    | "custom_tool_call_output"
            )
        })
}

pub(in super::super) fn should_skip_chat_live_text_event(event_type: &str, value: &Value) -> bool {
    match event_type {
        "response.output_text.done"
        | "response.content_part.added"
        | "response.content_part.delta"
        | "response.content_part.done" => true,
        "response.output_item.added" | "response.output_item.done" => {
            !is_tool_related_output_item(value)
        }
        _ => false,
    }
}

pub(in super::super) fn should_skip_completion_live_text_event(
    event_type: &str,
    value: &Value,
) -> bool {
    match event_type {
        "response.output_text.done"
        | "response.content_part.added"
        | "response.content_part.delta"
        | "response.content_part.done" => true,
        "response.output_item.added" | "response.output_item.done" => {
            !is_tool_related_output_item(value)
        }
        _ => false,
    }
}

pub(in super::super) fn normalize_chat_chunk_delta_role(
    mapped: &mut Value,
    role_emitted: &mut bool,
) {
    let Some(choices) = mapped.get_mut("choices").and_then(Value::as_array_mut) else {
        return;
    };
    let mut saw_role = false;
    for choice in choices {
        let Some(delta) = choice.get_mut("delta").and_then(Value::as_object_mut) else {
            continue;
        };
        if delta.contains_key("role") {
            if *role_emitted {
                delta.remove("role");
            } else {
                saw_role = true;
            }
        }
    }
    if saw_role {
        *role_emitted = true;
    }
}

pub(in super::super) fn build_chat_fallback_content_chunk(
    meta: &OpenAIStreamMeta,
    content: &str,
) -> Value {
    json!({
        "id": meta.response_id.clone().unwrap_or_default(),
        "object": "chat.completion.chunk",
        "created": meta.created.unwrap_or(0),
        "model": meta.model.clone().unwrap_or_default(),
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "content": content
            },
            "finish_reason": Value::Null
        }]
    })
}

pub(in super::super) fn build_completion_fallback_text_chunk(
    meta: &OpenAIStreamMeta,
    text: &str,
) -> Value {
    json!({
        "id": meta.response_id.clone().unwrap_or_default(),
        "object": "text_completion",
        "created": meta.created.unwrap_or(0),
        "model": meta.model.clone().unwrap_or_default(),
        "choices": [{
            "index": 0,
            "text": text
        }]
    })
}

fn append_sse_data_frame(buffer: &mut String, payload: &Value) {
    let data = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
    buffer.push_str("data: ");
    buffer.push_str(data.as_str());
    buffer.push_str("\n\n");
}

fn collect_text_for_sse_delta(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    let mut text = String::new();
    collect_response_output_text(value, &mut text);
    text.trim().to_string()
}

pub(in super::super) fn synthesize_chat_completion_sse_from_json(value: &Value) -> Vec<u8> {
    let Some(root) = value.as_object() else {
        return b"data: [DONE]\n\n".to_vec();
    };
    if root.contains_key("error") {
        let mut out = String::new();
        append_sse_data_frame(&mut out, value);
        out.push_str("data: [DONE]\n\n");
        return out.into_bytes();
    }

    let id = root
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    let created = root.get("created").and_then(Value::as_i64).unwrap_or(0);
    let model = root
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();

    let mut out = String::new();
    let mut finish_reason = Value::String("stop".to_string());
    let usage = root.get("usage").cloned();

    let first_choice = root
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .cloned();
    if let Some(choice) = first_choice {
        if let Some(reason) = choice.get("finish_reason") {
            if !reason.is_null() {
                finish_reason = reason.clone();
            }
        }

        let mut delta = serde_json::Map::new();
        delta.insert("role".to_string(), Value::String("assistant".to_string()));
        let message = choice.get("message");
        let content = collect_text_for_sse_delta(message.and_then(|msg| msg.get("content")));
        if !content.is_empty() {
            delta.insert("content".to_string(), Value::String(content));
        }
        if let Some(tool_calls) = message
            .and_then(|msg| msg.get("tool_calls"))
            .and_then(Value::as_array)
            .filter(|tool_calls| !tool_calls.is_empty())
        {
            delta.insert("tool_calls".to_string(), Value::Array(tool_calls.to_vec()));
        }
        if delta.get("content").is_some() || delta.get("tool_calls").is_some() {
            let content_chunk = json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": Value::Object(delta),
                    "finish_reason": Value::Null
                }]
            });
            append_sse_data_frame(&mut out, &content_chunk);
        }
    }

    let mut finish_chunk = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": finish_reason
        }]
    });
    if let Some(usage) = usage {
        if let Some(finish_obj) = finish_chunk.as_object_mut() {
            finish_obj.insert("usage".to_string(), usage);
        }
    }
    append_sse_data_frame(&mut out, &finish_chunk);
    out.push_str("data: [DONE]\n\n");
    out.into_bytes()
}

pub(in super::super) fn synthesize_completions_sse_from_json(value: &Value) -> Vec<u8> {
    let Some(root) = value.as_object() else {
        return b"data: [DONE]\n\n".to_vec();
    };
    if root.contains_key("error") {
        let mut out = String::new();
        append_sse_data_frame(&mut out, value);
        out.push_str("data: [DONE]\n\n");
        return out.into_bytes();
    }

    let id = root
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    let created = root.get("created").and_then(Value::as_i64).unwrap_or(0);
    let model = root
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();

    let mut out = String::new();
    let mut finish_reason = Value::String("stop".to_string());
    let usage = root.get("usage").cloned();

    let first_choice = root
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .cloned();
    if let Some(choice) = first_choice {
        if let Some(reason) = choice.get("finish_reason") {
            if !reason.is_null() {
                finish_reason = reason.clone();
            }
        }
        let text = collect_text_for_sse_delta(choice.get("text"));
        if !text.is_empty() {
            let content_chunk = json!({
                "id": id,
                "object": "text_completion",
                "created": created,
                "model": model,
                "choices": [{
                    "index": 0,
                    "text": text,
                    "finish_reason": Value::Null
                }]
            });
            append_sse_data_frame(&mut out, &content_chunk);
        }
    }

    let mut finish_chunk = json!({
        "id": id,
        "object": "text_completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "text": "",
            "finish_reason": finish_reason
        }]
    });
    if let Some(usage) = usage {
        if let Some(finish_obj) = finish_chunk.as_object_mut() {
            finish_obj.insert("usage".to_string(), usage);
        }
    }
    append_sse_data_frame(&mut out, &finish_chunk);
    out.push_str("data: [DONE]\n\n");
    out.into_bytes()
}
