use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

mod anthropic;
mod openai;

type ToolNameRestoreMap = super::ToolNameRestoreMap;

const DEFAULT_ANTHROPIC_MODEL: &str = "gpt-5.3-codex";
const DEFAULT_ANTHROPIC_REASONING: &str = "high";
const DEFAULT_ANTHROPIC_INSTRUCTIONS: &str =
    "You are Codex, a coding assistant that responds clearly and safely.";
pub(super) use self::anthropic::convert_anthropic_messages_request;
use self::openai::shorten_openai_tool_name_with_map;
pub(super) use self::openai::{
    convert_openai_chat_completions_request, convert_openai_completions_request,
};

fn resolve_prompt_cache_key(
    obj: &serde_json::Map<String, Value>,
    model: Option<&Value>,
) -> Option<String> {
    super::prompt_cache::resolve_prompt_cache_key(obj, model)
}

fn build_shortened_tool_name_maps<I>(names: I) -> (BTreeMap<String, String>, ToolNameRestoreMap)
where
    I: IntoIterator<Item = String>,
{
    let mut unique_names = BTreeSet::new();
    for name in names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        unique_names.insert(trimmed.to_string());
    }

    let mut used = BTreeSet::new();
    let mut tool_name_map = BTreeMap::new();
    let mut restore_map = ToolNameRestoreMap::new();
    for original in unique_names {
        let base = openai::shorten_openai_tool_name_candidate(original.as_str());
        let mut candidate = base.clone();
        let mut suffix = 1usize;
        while used.contains(&candidate) {
            let suffix_text = format!("_{suffix}");
            let mut truncated = base.clone();
            let limit = openai::MAX_OPENAI_TOOL_NAME_LEN.saturating_sub(suffix_text.len());
            if truncated.len() > limit {
                truncated = truncated.chars().take(limit).collect();
            }
            candidate = format!("{truncated}{suffix_text}");
            suffix += 1;
        }
        used.insert(candidate.clone());
        if original != candidate {
            restore_map.insert(candidate.clone(), original.clone());
        }
        tool_name_map.insert(original, candidate);
    }
    (tool_name_map, restore_map)
}

fn convert_chat_messages_to_responses_input(
    messages: &[Value],
    tool_name_map: &BTreeMap<String, String>,
) -> Result<(Option<String>, Vec<Value>), String> {
    let mut instructions_parts = Vec::new();
    let mut input_items = Vec::new();

    for message in messages {
        let Some(message_obj) = message.as_object() else {
            continue;
        };
        let Some(role) = message_obj.get("role").and_then(Value::as_str) else {
            continue;
        };
        match role {
            "system" => {
                if let Some(content) = message_obj.get("content").and_then(Value::as_str) {
                    if !content.trim().is_empty() {
                        instructions_parts.push(content.to_string());
                    }
                }
            }
            "user" => {
                if let Some(content) = message_obj.get("content") {
                    let content_items = convert_user_message_content_to_responses_items(content);
                    if !content_items.is_empty() {
                        input_items.push(json!({
                            "type": "message",
                            "role": "user",
                            "content": content_items
                        }));
                    }
                }
            }
            "assistant" => {
                if let Some(content) = message_obj.get("content") {
                    append_assistant_content_to_responses_input(
                        &mut input_items,
                        content,
                        tool_name_map,
                    )?;
                }
                if let Some(tool_calls) = message_obj.get("tool_calls").and_then(Value::as_array) {
                    for (index, tool_call) in tool_calls.iter().enumerate() {
                        let Some(tool_obj) = tool_call.as_object() else {
                            continue;
                        };
                        let call_id = tool_obj
                            .get("id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("call_{index}"));
                        let Some(function_name) = tool_obj
                            .get("function")
                            .and_then(|value| value.get("name"))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                        else {
                            continue;
                        };
                        let function_name =
                            shorten_openai_tool_name_with_map(function_name, tool_name_map);
                        let arguments = tool_obj
                            .get("function")
                            .and_then(|value| value.get("arguments"))
                            .map(|value| {
                                if let Some(text) = value.as_str() {
                                    text.to_string()
                                } else {
                                    serde_json::to_string(value)
                                        .unwrap_or_else(|_| "{}".to_string())
                                }
                            })
                            .unwrap_or_else(|| "{}".to_string());
                        input_items.push(json!({
                            "type": "function_call",
                            "call_id": call_id,
                            "name": function_name,
                            "arguments": arguments
                        }));
                    }
                }
            }
            "tool" => {
                let call_id = message_obj
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "tool role message missing tool_call_id".to_string())?;
                let output =
                    convert_tool_message_content_to_responses_output(message_obj.get("content"))?;
                input_items.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": output
                }));
            }
            _ => {}
        }
    }

    let instructions = if instructions_parts.is_empty() {
        None
    } else {
        Some(instructions_parts.join("\n\n"))
    };
    Ok((instructions, input_items))
}

fn append_assistant_content_to_responses_input(
    input_items: &mut Vec<Value>,
    content: &Value,
    tool_name_map: &BTreeMap<String, String>,
) -> Result<(), String> {
    if let Some(text) = content.as_str() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            input_items.push(json!({
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": trimmed }]
            }));
        }
        return Ok(());
    }

    let items = if let Some(array) = content.as_array() {
        array.to_vec()
    } else if content.is_object() {
        vec![content.clone()]
    } else if content.is_null() {
        Vec::new()
    } else {
        return Err("unsupported assistant content".to_string());
    };

    let mut pending_parts = Vec::new();
    for item in items {
        let Some(item_obj) = item.as_object() else {
            continue;
        };
        let item_type = item_obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match item_type {
            "text" | "output_text" => {
                if let Some(text) = item_obj.get("text").and_then(Value::as_str) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        pending_parts.push(json!({
                            "type": "output_text",
                            "text": trimmed,
                        }));
                    }
                }
            }
            "tool_use" => {
                flush_assistant_output_parts(input_items, &mut pending_parts);
                let Some(function_name) = item_obj
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                let function_name = shorten_openai_tool_name_with_map(function_name, tool_name_map);
                let call_id = item_obj
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| item_obj.get("call_id").and_then(Value::as_str))
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("call_0");
                let arguments = serde_json::to_string(
                    &item_obj.get("input").cloned().unwrap_or_else(|| json!({})),
                )
                .map_err(|err| format!("serialize assistant tool_use input failed: {err}"))?;
                input_items.push(json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": function_name,
                    "arguments": arguments
                }));
            }
            _ => continue,
        }
    }
    flush_assistant_output_parts(input_items, &mut pending_parts);
    Ok(())
}

fn flush_assistant_output_parts(input_items: &mut Vec<Value>, pending_parts: &mut Vec<Value>) {
    if pending_parts.is_empty() {
        return;
    }
    input_items.push(json!({
        "type": "message",
        "role": "assistant",
        "content": pending_parts.clone(),
    }));
    pending_parts.clear();
}

fn convert_tool_message_content_to_responses_output(
    value: Option<&Value>,
) -> Result<Value, String> {
    let Some(value) = value else {
        return Ok(Value::String(String::new()));
    };
    if value.is_null() {
        return Ok(Value::String(String::new()));
    }
    if let Some(text) = value.as_str() {
        return Ok(Value::String(text.to_string()));
    }
    if let Some(items) = value.as_array() {
        let mapped_items = items
            .iter()
            .filter_map(map_tool_result_content_item_to_responses_output_item)
            .collect::<Vec<_>>();
        if mapped_items.is_empty() {
            return Ok(Value::String(String::new()));
        }
        return Ok(Value::Array(mapped_items));
    }
    if let Some(item) = map_tool_result_content_item_to_responses_output_item(value) {
        return Ok(Value::Array(vec![item]));
    }
    serde_json::to_string(value)
        .map(Value::String)
        .map_err(|err| format!("serialize tool result content failed: {err}"))
}

fn map_tool_result_content_item_to_responses_output_item(item: &Value) -> Option<Value> {
    if let Some(text) = item.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(json!({
            "type": "input_text",
            "text": trimmed,
        }));
    }

    let obj = item.as_object()?;
    let item_type = obj.get("type").and_then(Value::as_str).unwrap_or_default();
    match item_type {
        "text" | "input_text" => obj
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|text| {
                json!({
                    "type": "input_text",
                    "text": text,
                })
            }),
        "input_image" => {
            let mut mapped = serde_json::Map::new();
            mapped.insert("type".to_string(), Value::String("input_image".to_string()));
            if let Some(image_url) = obj.get("image_url").cloned() {
                mapped.insert("image_url".to_string(), image_url);
            } else if let Some(file_id) = obj.get("file_id").cloned() {
                mapped.insert("file_id".to_string(), file_id);
            } else {
                return None;
            }
            Some(Value::Object(mapped))
        }
        "image" => map_anthropic_image_block_to_responses_item(obj),
        _ => serde_json::to_string(item).ok().and_then(|text| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(json!({
                    "type": "input_text",
                    "text": trimmed,
                }))
            }
        }),
    }
}

fn prefix_tool_error_output(output: Value) -> Value {
    match output {
        Value::String(text) => Value::String(format!("[tool_error] {text}")),
        Value::Array(mut items) => {
            items.insert(
                0,
                json!({
                    "type": "input_text",
                    "text": "[tool_error]",
                }),
            );
            Value::Array(items)
        }
        other => other,
    }
}

fn convert_user_message_content_to_responses_items(content: &Value) -> Vec<Value> {
    match content {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Vec::new()
            } else {
                vec![json!({
                    "type": "input_text",
                    "text": trimmed,
                })]
            }
        }
        Value::Array(items) => items
            .iter()
            .filter_map(map_user_content_item_to_responses_item)
            .collect(),
        Value::Null => Vec::new(),
        other => {
            let text = serde_json::to_string(other).unwrap_or_default();
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Vec::new()
            } else {
                vec![json!({
                    "type": "input_text",
                    "text": trimmed,
                })]
            }
        }
    }
}

fn map_user_content_item_to_responses_item(item: &Value) -> Option<Value> {
    if let Some(text) = item.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(json!({
            "type": "input_text",
            "text": trimmed,
        }));
    }

    let obj = item.as_object()?;
    let item_type = obj.get("type").and_then(Value::as_str).unwrap_or_default();
    match item_type {
        "text" | "input_text" | "output_text" => obj
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|text| {
                json!({
                    "type": "input_text",
                    "text": text,
                })
            }),
        "input_image" => {
            let mut mapped = serde_json::Map::new();
            mapped.insert("type".to_string(), Value::String("input_image".to_string()));
            if let Some(image_url) = obj.get("image_url").cloned() {
                mapped.insert("image_url".to_string(), image_url);
            } else if let Some(file_id) = obj.get("file_id").cloned() {
                mapped.insert("file_id".to_string(), file_id);
            } else {
                return None;
            }
            Some(Value::Object(mapped))
        }
        "image_url" => extract_openai_image_url(obj).map(|image_url| {
            json!({
                "type": "input_image",
                "image_url": image_url,
            })
        }),
        _ => None,
    }
}

fn extract_openai_image_url(obj: &serde_json::Map<String, Value>) -> Option<String> {
    if let Some(text) = obj.get("image_url").and_then(Value::as_str) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    let image_url_obj = obj.get("image_url").and_then(Value::as_object)?;
    image_url_obj
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn map_anthropic_image_block_to_responses_item(
    block: &serde_json::Map<String, Value>,
) -> Option<Value> {
    let source = block.get("source")?;
    let source_obj = source.as_object()?;

    if let Some(image_url) = source_obj
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(json!({
            "type": "input_image",
            "image_url": image_url,
        }));
    }

    let source_type = source_obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if source_type == "base64" || source_obj.contains_key("data") {
        let media_type = source_obj
            .get("media_type")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("image/png");
        let data = source_obj
            .get("data")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        return Some(json!({
            "type": "input_image",
            "image_url": format!("data:{media_type};base64,{data}"),
        }));
    }

    if let Some(file_id) = source_obj
        .get("file_id")
        .or_else(|| source_obj.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(json!({
            "type": "input_image",
            "file_id": file_id,
        }));
    }

    None
}

fn extract_tool_result_output(value: Option<&Value>) -> Result<Value, String> {
    convert_tool_message_content_to_responses_output(value)
}

fn map_anthropic_tool_definition(
    value: &Value,
    tool_name_map: &BTreeMap<String, String>,
) -> Option<Value> {
    let Some(obj) = value.as_object() else {
        return None;
    };
    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| obj.get("type").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let description = obj
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let parameters = obj
        .get("input_schema")
        .cloned()
        .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
    let parameters = fix_array_items_in_schema(parameters);
    let mapped_name = shorten_openai_tool_name_with_map(name, tool_name_map);

    let mut tool_obj = serde_json::Map::new();
    tool_obj.insert("type".to_string(), Value::String("function".to_string()));
    tool_obj.insert("name".to_string(), Value::String(mapped_name));
    if !description.is_empty() {
        tool_obj.insert("description".to_string(), Value::String(description));
    }
    tool_obj.insert("parameters".to_string(), parameters);

    Some(Value::Object(tool_obj))
}

fn map_anthropic_tool_choice(
    value: &Value,
    tool_name_map: &BTreeMap<String, String>,
) -> Option<Value> {
    if let Some(text) = value.as_str() {
        return Some(Value::String(text.to_string()));
    }
    let Some(obj) = value.as_object() else {
        return None;
    };
    let choice_type = obj.get("type").and_then(Value::as_str).unwrap_or("auto");
    match choice_type {
        "auto" => Some(Value::String("auto".to_string())),
        "any" => Some(Value::String("required".to_string())),
        "none" => Some(Value::String("none".to_string())),
        "tool" => {
            let name = obj
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())?;
            let mapped_name = shorten_openai_tool_name_with_map(name, tool_name_map);
            Some(json!({
                "type": "function",
                "name": mapped_name
            }))
        }
        _ => None,
    }
}

fn resolve_anthropic_parallel_tool_calls(source: &serde_json::Map<String, Value>) -> bool {
    !source
        .get("tool_choice")
        .and_then(Value::as_object)
        .and_then(|tool_choice| tool_choice.get("disable_parallel_tool_use"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// OpenAI/Codex 后端对 JSON Schema 的要求比 Anthropic 更严格：
/// - `"type":"array"` 必须携带 `"items"` 定义
/// - `"type":"object"` 必须携带 `"properties"` 定义
/// 此函数递归补齐缺失的字段。
fn fix_array_items_in_schema(mut value: Value) -> Value {
    match &mut value {
        Value::Object(obj) => {
            let schema_type = obj.get("type").and_then(Value::as_str).map(str::to_string);
            if schema_type.as_deref() == Some("array") && !obj.contains_key("items") {
                obj.insert("items".to_string(), json!({}));
            }
            if schema_type.as_deref() == Some("object") && !obj.contains_key("properties") {
                obj.insert("properties".to_string(), json!({}));
            }
            for (_, v) in obj.iter_mut() {
                *v = fix_array_items_in_schema(v.clone());
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                *v = fix_array_items_in_schema(v.clone());
            }
        }
        _ => {}
    }
    value
}
