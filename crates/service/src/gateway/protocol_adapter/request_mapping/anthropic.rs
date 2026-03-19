use serde_json::{json, Value};

pub(crate) fn convert_anthropic_messages_request(
    body: &[u8],
) -> Result<(Vec<u8>, bool, super::ToolNameRestoreMap), String> {
    let payload: Value =
        serde_json::from_slice(body).map_err(|_| "invalid claude request json".to_string())?;
    let Some(obj) = payload.as_object() else {
        return Err("claude request body must be an object".to_string());
    };

    let mut messages = Vec::new();

    if let Some(system) = obj.get("system") {
        let system_text = extract_text_content(system)?;
        if !system_text.trim().is_empty() {
            messages.push(json!({
                "role": "system",
                "content": system_text,
            }));
        }
    }

    let source_messages = obj
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "claude messages field is required".to_string())?;
    for message in source_messages {
        let Some(message_obj) = message.as_object() else {
            return Err("invalid claude message item".to_string());
        };
        let role = message_obj
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| "claude message role is required".to_string())?;
        let content = message_obj
            .get("content")
            .ok_or_else(|| "claude message content is required".to_string())?;
        match role {
            "assistant" => append_assistant_messages(&mut messages, content)?,
            "user" => append_user_messages(&mut messages, content)?,
            "tool" => append_tool_role_message(&mut messages, message_obj, content)?,
            other => return Err(format!("unsupported claude message role: {other}")),
        }
    }

    let (tool_name_map, tool_name_restore_map) =
        super::build_shortened_tool_name_maps(collect_anthropic_tool_names(obj, source_messages));
    let (instructions, input_items) =
        super::convert_chat_messages_to_responses_input(&messages, &tool_name_map)?;
    let mut out = serde_json::Map::new();
    let resolved_model = resolve_anthropic_upstream_model(obj);
    out.insert("model".to_string(), Value::String(resolved_model));
    let resolved_instructions = instructions
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(super::DEFAULT_ANTHROPIC_INSTRUCTIONS);
    out.insert(
        "instructions".to_string(),
        Value::String(resolved_instructions.to_string()),
    );
    out.insert(
        "text".to_string(),
        json!({
            "format": {
                "type": "text",
            }
        }),
    );
    let resolved_reasoning = resolve_anthropic_reasoning_effort(obj).to_string();
    let mut reasoning = serde_json::Map::new();
    reasoning.insert(
        "effort".to_string(),
        Value::String(resolved_reasoning.clone()),
    );
    if let Some(summary) = resolve_anthropic_reasoning_summary(obj) {
        reasoning.insert("summary".to_string(), Value::String(summary.to_string()));
    }
    out.insert("reasoning".to_string(), Value::Object(reasoning));
    out.insert("input".to_string(), Value::Array(input_items));
    if let Some(encrypted_content) = extract_latest_anthropic_thinking_signature(source_messages) {
        out.insert(
            "encrypted_content".to_string(),
            Value::String(encrypted_content),
        );
    }

    if let Some(prompt_cache_key) = super::resolve_prompt_cache_key(obj, out.get("model")) {
        out.insert(
            "prompt_cache_key".to_string(),
            Value::String(prompt_cache_key),
        );
    }
    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        let mapped_tools = tools
            .iter()
            .filter_map(|tool| super::map_anthropic_tool_definition(tool, &tool_name_map))
            .collect::<Vec<_>>();
        if !mapped_tools.is_empty() {
            out.insert("tools".to_string(), Value::Array(mapped_tools));
            if !obj.contains_key("tool_choice") {
                out.insert("tool_choice".to_string(), Value::String("auto".to_string()));
            }
        }
    }
    if let Some(tool_choice) = obj.get("tool_choice") {
        if !tool_choice.is_null() {
            if let Some(mapped_tool_choice) =
                super::map_anthropic_tool_choice(tool_choice, &tool_name_map)
            {
                out.insert("tool_choice".to_string(), mapped_tool_choice);
            }
        }
    }
    let request_stream = obj.get("stream").and_then(Value::as_bool).unwrap_or(true);
    out.insert("stream".to_string(), Value::Bool(true));
    out.insert(
        "parallel_tool_calls".to_string(),
        Value::Bool(super::resolve_anthropic_parallel_tool_calls(obj)),
    );
    out.insert("store".to_string(), Value::Bool(false));
    out.insert(
        "include".to_string(),
        Value::Array(vec![Value::String(
            "reasoning.encrypted_content".to_string(),
        )]),
    );

    serde_json::to_vec(&Value::Object(out))
        .map(|bytes| (bytes, request_stream, tool_name_restore_map))
        .map_err(|err| format!("convert claude request failed: {err}"))
}

fn collect_anthropic_tool_names(
    obj: &serde_json::Map<String, Value>,
    source_messages: &[Value],
) -> Vec<String> {
    let mut names = Vec::new();

    if let Some(tools) = obj.get("tools").and_then(Value::as_array) {
        for tool in tools {
            let Some(tool_obj) = tool.as_object() else {
                continue;
            };
            let Some(name) = tool_obj
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| tool_obj.get("type").and_then(Value::as_str))
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            names.push(name.to_string());
        }
    }

    if let Some(name) = obj
        .get("tool_choice")
        .and_then(Value::as_object)
        .and_then(|tool_choice| {
            if tool_choice.get("type").and_then(Value::as_str) != Some("tool") {
                return None;
            }
            tool_choice
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
    {
        names.push(name.to_string());
    }

    for message in source_messages {
        let Some(message_obj) = message.as_object() else {
            continue;
        };
        let Some(content) = message_obj.get("content") else {
            continue;
        };
        let items = if let Some(array) = content.as_array() {
            array
        } else {
            continue;
        };
        for item in items {
            let Some(item_obj) = item.as_object() else {
                continue;
            };
            if item_obj.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let Some(name) = item_obj
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            names.push(name.to_string());
        }
    }

    names
}

fn resolve_anthropic_upstream_model(source: &serde_json::Map<String, Value>) -> String {
    let requested_model = source
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match requested_model {
        Some(model) if model.to_ascii_lowercase().contains("codex") => model.to_string(),
        _ => super::DEFAULT_ANTHROPIC_MODEL.to_string(),
    }
}

fn resolve_anthropic_reasoning_effort(source: &serde_json::Map<String, Value>) -> &'static str {
    source
        .get("reasoning")
        .and_then(Value::as_object)
        .and_then(|value| value.get("effort"))
        .and_then(Value::as_str)
        .or_else(|| {
            source
                .get("output_config")
                .and_then(Value::as_object)
                .and_then(|value| value.get("effort"))
                .and_then(Value::as_str)
        })
        .or_else(|| source.get("effort").and_then(Value::as_str))
        .and_then(crate::reasoning_effort::normalize_reasoning_effort)
        .unwrap_or(super::DEFAULT_ANTHROPIC_REASONING)
}

fn resolve_anthropic_reasoning_summary(
    source: &serde_json::Map<String, Value>,
) -> Option<&'static str> {
    match source.get("thinking") {
        Some(Value::Bool(true)) => Some("detailed"),
        Some(Value::Bool(false)) => Some("none"),
        Some(Value::String(text)) => match text.trim().to_ascii_lowercase().as_str() {
            "enabled" | "on" | "true" => Some("detailed"),
            "disabled" | "off" | "false" => Some("none"),
            _ => None,
        },
        Some(Value::Object(obj)) => {
            let thinking_type = obj
                .get("type")
                .and_then(Value::as_str)
                .map(|value| value.trim().to_ascii_lowercase());
            match thinking_type.as_deref() {
                Some("disabled") => Some("none"),
                Some("enabled") => Some("detailed"),
                _ if obj
                    .get("budget_tokens")
                    .and_then(Value::as_i64)
                    .is_some_and(|value| value > 0) =>
                {
                    Some("detailed")
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn extract_latest_anthropic_thinking_signature(messages: &[Value]) -> Option<String> {
    for message in messages.iter().rev() {
        let Some(message_obj) = message.as_object() else {
            continue;
        };
        let Some(content) = message_obj.get("content") else {
            continue;
        };
        let blocks = if let Some(array) = content.as_array() {
            array
        } else if content.is_object() {
            std::slice::from_ref(content)
        } else {
            continue;
        };
        for block in blocks.iter().rev() {
            let Some(block_obj) = block.as_object() else {
                continue;
            };
            let block_type = block_obj
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !matches!(block_type, "thinking" | "redacted_thinking") {
                continue;
            }
            let signature = block_obj
                .get("signature")
                .or_else(|| block_obj.get("encrypted_content"))
                .or_else(|| block_obj.get("data"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if let Some(signature) = signature {
                return Some(signature.to_string());
            }
        }
    }
    None
}

fn append_assistant_messages(messages: &mut Vec<Value>, content: &Value) -> Result<(), String> {
    if let Some(text) = content.as_str() {
        messages.push(json!({
            "role": "assistant",
            "content": text,
        }));
        return Ok(());
    }

    let blocks = if let Some(array) = content.as_array() {
        array.to_vec()
    } else if content.is_object() {
        vec![content.clone()]
    } else {
        return Err("unsupported assistant content".to_string());
    };

    let mut content_parts = Vec::new();
    for block in blocks {
        let Some(block_obj) = block.as_object() else {
            return Err("invalid assistant content block".to_string());
        };
        let block_type = block_obj
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| "assistant content block missing type".to_string())?;
        match block_type {
            "text" => {
                if let Some(text) = block_obj.get("text").and_then(Value::as_str) {
                    if !text.trim().is_empty() {
                        content_parts.push(json!({
                            "type": "text",
                            "text": text,
                        }));
                    }
                }
            }
            "tool_use" => {
                let id = block_obj
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("toolu_{}", content_parts.len()));
                let Some(name) = block_obj
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                else {
                    continue;
                };
                content_parts.push(json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": block_obj.get("input").cloned().unwrap_or_else(|| json!({})),
                }));
            }
            _ => continue,
        }
    }

    if content_parts.is_empty() {
        return Ok(());
    }
    messages.push(json!({
        "role": "assistant",
        "content": content_parts,
    }));
    Ok(())
}

fn append_user_messages(messages: &mut Vec<Value>, content: &Value) -> Result<(), String> {
    if let Some(text) = content.as_str() {
        if !text.trim().is_empty() {
            messages.push(json!({
                "role": "user",
                "content": text,
            }));
        }
        return Ok(());
    }

    let blocks = if let Some(array) = content.as_array() {
        array.to_vec()
    } else if content.is_object() {
        vec![content.clone()]
    } else {
        return Err("unsupported user content".to_string());
    };

    let mut pending_parts = Vec::new();
    for block in blocks {
        let Some(block_obj) = block.as_object() else {
            return Err("invalid user content block".to_string());
        };
        let block_type = block_obj
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| "user content block missing type".to_string())?;
        match block_type {
            "text" => {
                if let Some(text) = block_obj.get("text").and_then(Value::as_str) {
                    if !text.trim().is_empty() {
                        pending_parts.push(json!({
                            "type": "input_text",
                            "text": text,
                        }));
                    }
                }
            }
            "image" => {
                if let Some(image_item) =
                    super::map_anthropic_image_block_to_responses_item(block_obj)
                {
                    pending_parts.push(image_item);
                }
            }
            "tool_result" => {
                flush_user_content_parts(messages, &mut pending_parts);
                let tool_use_id = block_obj
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .or_else(|| block_obj.get("id").and_then(Value::as_str))
                    .unwrap_or_default();
                if tool_use_id.is_empty() {
                    continue;
                }
                let mut tool_content = super::extract_tool_result_output(block_obj.get("content"))?;
                if block_obj
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    tool_content = super::prefix_tool_error_output(tool_content);
                }
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": tool_content,
                }));
            }
            _ => continue,
        }
    }
    flush_user_content_parts(messages, &mut pending_parts);
    Ok(())
}

fn append_tool_role_message(
    messages: &mut Vec<Value>,
    message_obj: &serde_json::Map<String, Value>,
    content: &Value,
) -> Result<(), String> {
    let tool_call_id = message_obj
        .get("tool_call_id")
        .or_else(|| message_obj.get("tool_use_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| "tool role message missing tool_call_id".to_string())?;
    let tool_content = super::extract_tool_result_output(Some(content))?;
    messages.push(json!({
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": tool_content,
    }));
    Ok(())
}

fn flush_user_content_parts(messages: &mut Vec<Value>, pending_parts: &mut Vec<Value>) {
    if pending_parts.is_empty() {
        return;
    }
    messages.push(json!({
        "role": "user",
        "content": pending_parts.clone(),
    }));
    pending_parts.clear();
}

fn extract_text_content(value: &Value) -> Result<String, String> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_string());
    }

    if let Some(block) = value.as_object() {
        return extract_text_from_block(block);
    }

    if let Some(array) = value.as_array() {
        let mut parts = Vec::new();
        for item in array {
            let Some(block) = item.as_object() else {
                return Err("invalid claude content block".to_string());
            };
            parts.push(extract_text_from_block(block)?);
        }
        return Ok(parts.join(""));
    }

    Err("unsupported claude content".to_string())
}

fn extract_text_from_block(block: &serde_json::Map<String, Value>) -> Result<String, String> {
    let block_type = block
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "claude content block missing type".to_string())?;
    if block_type != "text" {
        return Err(format!(
            "unsupported claude content block type: {block_type}"
        ));
    }
    block
        .get("text")
        .and_then(Value::as_str)
        .map(|v| v.to_string())
        .ok_or_else(|| "claude text block missing text".to_string())
}
