use serde_json::{json, Map, Value};

use super::openai_chat::{
    extract_chat_content_text, extract_stream_event_text, map_openai_response_to_chat_completion,
};

fn map_chat_choice_to_completion(choice: &Map<String, Value>, default_index: usize) -> Value {
    let mut out = Map::new();
    let index = choice
        .get("index")
        .and_then(Value::as_i64)
        .unwrap_or(default_index as i64);
    out.insert("index".to_string(), Value::Number(index.into()));
    let text = extract_chat_content_text(
        choice
            .get("text")
            .or_else(|| choice.get("message").and_then(|v| v.get("content")))
            .or_else(|| choice.get("delta").and_then(|v| v.get("content"))),
    );
    out.insert("text".to_string(), Value::String(text));
    if let Some(finish_reason) = choice.get("finish_reason") {
        if !finish_reason.is_null() {
            out.insert("finish_reason".to_string(), finish_reason.clone());
        }
    }
    if let Some(logprobs) = choice.get("logprobs") {
        out.insert("logprobs".to_string(), logprobs.clone());
    }
    Value::Object(out)
}

fn map_chat_response_to_completions(value: &Value) -> Value {
    if value
        .get("object")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind == "text_completion")
    {
        return value.clone();
    }

    let id = value
        .get("id")
        .cloned()
        .unwrap_or_else(|| Value::String(String::new()));
    let created = value
        .get("created")
        .cloned()
        .unwrap_or_else(|| Value::Number(0.into()));
    let model = value
        .get("model")
        .cloned()
        .unwrap_or_else(|| Value::String(String::new()));

    let mut out = Map::new();
    out.insert("id".to_string(), id);
    out.insert(
        "object".to_string(),
        Value::String("text_completion".to_string()),
    );
    out.insert("created".to_string(), created);
    out.insert("model".to_string(), model);

    let mut choices = Vec::new();
    if let Some(arr) = value.get("choices").and_then(Value::as_array) {
        for (idx, choice) in arr.iter().enumerate() {
            if let Some(choice_obj) = choice.as_object() {
                choices.push(map_chat_choice_to_completion(choice_obj, idx));
            }
        }
    }
    out.insert("choices".to_string(), Value::Array(choices));

    if let Some(usage) = value.get("usage") {
        out.insert("usage".to_string(), usage.clone());
    } else if let Some(usage) = value
        .get("response")
        .and_then(|response| response.get("usage"))
    {
        out.insert("usage".to_string(), usage.clone());
    }
    Value::Object(out)
}

fn build_openai_completions_text_chunk(value: &Value, text: &str) -> Value {
    json!({
        "id": super::stream_event_response_id(value),
        "object": "text_completion",
        "created": super::stream_event_created(value),
        "model": super::stream_event_model(value),
        "choices": [{
            "index": 0,
            "text": text
        }]
    })
}

pub(crate) fn convert_openai_completions_stream_chunk(value: &Value) -> Option<Value> {
    if let Some(chunk_type) = value.get("type").and_then(Value::as_str) {
        match chunk_type {
            "response.output_text.delta"
            | "response.output_text.done"
            | "response.output_item.added"
            | "response.content_part.added"
            | "response.content_part.delta"
            | "response.content_part.done"
            | "response.output_item.done" => {
                let is_tool_call_item = value
                    .get("item")
                    .or_else(|| value.get("output_item"))
                    .and_then(|item| item.get("type"))
                    .and_then(Value::as_str)
                    .is_some_and(|item_type| item_type == "function_call");
                if is_tool_call_item {
                    return None;
                }
                let text = extract_stream_event_text(value);
                if text.is_empty() {
                    return None;
                }
                return Some(build_openai_completions_text_chunk(value, text.as_str()));
            }
            "response.completed" | "response.done" => {
                let response = value.get("response").unwrap_or(&Value::Null);
                let usage = response
                    .get("usage")
                    .cloned()
                    .or_else(|| value.get("usage").cloned());
                let mut out = json!({
                    "id": response.get("id").and_then(Value::as_str).unwrap_or(""),
                    "object": "text_completion",
                    "created": response.get("created").and_then(Value::as_i64).unwrap_or(0),
                    "model": response.get("model").and_then(Value::as_str).unwrap_or(""),
                    "choices": [{
                        "index": 0,
                        "text": "",
                        "finish_reason": "stop"
                    }]
                });
                if let Some(usage) = usage {
                    if let Some(out_obj) = out.as_object_mut() {
                        out_obj.insert("usage".to_string(), usage);
                    }
                }
                return Some(out);
            }
            _ => {}
        }

        if chunk_type == "response.function_call_arguments.delta"
            || chunk_type == "response.function_call_arguments.done"
            || chunk_type == "response.custom_tool_call_input.delta"
            || chunk_type == "response.custom_tool_call_input.done"
        {
            return None;
        }
        let text = extract_stream_event_text(value);
        if !text.is_empty() {
            return Some(build_openai_completions_text_chunk(value, text.as_str()));
        }
    }

    if value
        .get("object")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind == "chat.completion.chunk")
    {
        let mapped = map_chat_response_to_completions(value);
        let has_usage = mapped.get("usage").is_some();
        let has_meaningful_choice =
            mapped
                .get("choices")
                .and_then(Value::as_array)
                .is_some_and(|choices| {
                    choices.iter().any(|choice| {
                        choice
                            .get("text")
                            .and_then(Value::as_str)
                            .is_some_and(|text| !text.is_empty())
                            || choice.get("finish_reason").is_some()
                    })
                });
        if has_usage || has_meaningful_choice {
            return Some(mapped);
        }
        return None;
    }

    if value.get("choices").is_some() || value.get("usage").is_some() {
        return Some(map_chat_response_to_completions(value));
    }
    None
}

pub(super) fn convert_openai_json_to_completions(
    body: &[u8],
) -> Result<(Vec<u8>, &'static str), String> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "invalid upstream json payload".to_string())?;
    let mapped = if value
        .get("object")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind == "response")
        || value.get("output").is_some()
    {
        let chat_mapped = map_openai_response_to_chat_completion(&value, None);
        map_chat_response_to_completions(&chat_mapped)
    } else {
        map_chat_response_to_completions(&value)
    };
    let bytes = serde_json::to_vec(&mapped)
        .map_err(|err| format!("serialize completions json failed: {err}"))?;
    Ok((bytes, "application/json"))
}

pub(super) fn convert_openai_sse_to_completions_json(
    body: &[u8],
) -> Result<(Vec<u8>, &'static str), String> {
    let text = std::str::from_utf8(body).map_err(|_| "invalid upstream sse bytes".to_string())?;
    let mut id = String::new();
    let mut model = String::new();
    let mut created: i64 = 0;
    let mut text_out = String::new();
    let mut finish_reason: Option<Value> = None;
    let mut usage: Option<Value> = None;
    let mut completed_response: Option<Value> = None;
    let mut data_lines = Vec::<String>::new();
    let mut event_name: Option<String> = None;
    let mut saw_text_delta = false;

    let flush_frame = |lines: &mut Vec<String>,
                       event_name: &mut Option<String>,
                       id: &mut String,
                       model: &mut String,
                       created: &mut i64,
                       text_out: &mut String,
                       finish_reason: &mut Option<Value>,
                       usage: &mut Option<Value>,
                       completed_response: &mut Option<Value>,
                       saw_text_delta: &mut bool| {
        if lines.is_empty() {
            *event_name = None;
            return;
        }
        let data = lines.join("\n");
        lines.clear();
        if data.trim() == "[DONE]" {
            *event_name = None;
            if finish_reason.is_none() {
                *finish_reason = Some(Value::String("stop".to_string()));
            }
            return;
        }
        let Some(value) = super::parse_openai_sse_event_value(&data, event_name.as_deref()) else {
            *event_name = None;
            return;
        };
        *event_name = None;
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if event_type == "response.output_text.delta" {
            *saw_text_delta = true;
        }
        if matches!(
            event_type,
            "response.output_text.done"
                | "response.content_part.added"
                | "response.content_part.delta"
                | "response.content_part.done"
        ) {
            return;
        }
        if matches!(
            event_type,
            "response.output_item.added" | "response.output_item.done"
        ) {
            let is_function_call_item = value
                .get("item")
                .or_else(|| value.get("output_item"))
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
                .is_some_and(|item_type| item_type == "function_call");
            if !is_function_call_item && *saw_text_delta {
                return;
            }
        }
        if value
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(super::is_response_completed_event_type)
        {
            if let Some(response) = value.get("response") {
                *completed_response = Some(response.clone());
            }
        }
        let Some(chunk) = convert_openai_completions_stream_chunk(&value) else {
            return;
        };
        if let Some(v) = chunk.get("id").and_then(Value::as_str) {
            if !v.is_empty() {
                *id = v.to_string();
            }
        }
        if let Some(v) = chunk.get("model").and_then(Value::as_str) {
            if !v.is_empty() {
                *model = v.to_string();
            }
        }
        if let Some(v) = chunk.get("created").and_then(Value::as_i64) {
            *created = v;
        }
        if let Some(chunk_usage) = chunk.get("usage") {
            *usage = Some(chunk_usage.clone());
        }
        if let Some(choices) = chunk.get("choices").and_then(Value::as_array) {
            for choice in choices {
                if let Some(text_piece) = choice.get("text").and_then(Value::as_str) {
                    text_out.push_str(text_piece);
                }
                if let Some(reason) = choice.get("finish_reason") {
                    if !reason.is_null() {
                        *finish_reason = Some(reason.clone());
                    }
                }
            }
        }
    };

    for line in text.lines() {
        if line.starts_with("event:") {
            event_name = Some(line[6..].trim_start().to_string());
            continue;
        }
        if line.starts_with("data:") {
            data_lines.push(line[5..].trim_start().to_string());
            continue;
        }
        if line.trim().is_empty() {
            flush_frame(
                &mut data_lines,
                &mut event_name,
                &mut id,
                &mut model,
                &mut created,
                &mut text_out,
                &mut finish_reason,
                &mut usage,
                &mut completed_response,
                &mut saw_text_delta,
            );
        }
    }
    flush_frame(
        &mut data_lines,
        &mut event_name,
        &mut id,
        &mut model,
        &mut created,
        &mut text_out,
        &mut finish_reason,
        &mut usage,
        &mut completed_response,
        &mut saw_text_delta,
    );

    if text_out.is_empty() {
        if let Some(response) = completed_response.as_ref() {
            let chat_completion = map_openai_response_to_chat_completion(response, None);
            let completion = map_chat_response_to_completions(&chat_completion);
            if let Some(v) = completion.get("id").and_then(Value::as_str) {
                if !v.is_empty() {
                    id = v.to_string();
                }
            }
            if let Some(v) = completion.get("model").and_then(Value::as_str) {
                if !v.is_empty() {
                    model = v.to_string();
                }
            }
            if let Some(v) = completion.get("created").and_then(Value::as_i64) {
                created = v;
            }
            if usage.is_none() {
                if let Some(v) = completion.get("usage") {
                    usage = Some(v.clone());
                }
            }
            if let Some(choice) = completion
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|choices| choices.first())
            {
                if let Some(v) = choice.get("text").and_then(Value::as_str) {
                    text_out.push_str(v);
                }
                if finish_reason.is_none() {
                    if let Some(v) = choice.get("finish_reason") {
                        if !v.is_null() {
                            finish_reason = Some(v.clone());
                        }
                    }
                }
            }
        }
    }

    let mut out = json!({
        "id": id,
        "object": "text_completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "text": text_out
        }]
    });
    if let Some(reason) = finish_reason {
        if let Some(choice_obj) = out
            .get_mut("choices")
            .and_then(Value::as_array_mut)
            .and_then(|choices| choices.get_mut(0))
            .and_then(Value::as_object_mut)
        {
            choice_obj.insert("finish_reason".to_string(), reason);
        }
    }
    if let Some(usage) = usage {
        if let Some(out_obj) = out.as_object_mut() {
            out_obj.insert("usage".to_string(), usage);
        }
    }

    let bytes = serde_json::to_vec(&out)
        .map_err(|err| format!("serialize completions json failed: {err}"))?;
    Ok((bytes, "application/json"))
}
