use serde_json::Value;

use super::request_rewrite_shared::{
    path_matches_template, retain_fields_by_templates, TemplateAllowlist,
};

pub(super) fn is_compact_path(path: &str) -> bool {
    path_matches_template(path, "/v1/responses/compact")
}

fn is_standard_responses_path(path: &str) -> bool {
    path_matches_template(path, "/v1/responses")
}

pub(super) fn is_responses_path(path: &str) -> bool {
    is_standard_responses_path(path) || is_compact_path(path)
}

pub(super) fn ensure_instructions(path: &str, obj: &mut serde_json::Map<String, Value>) -> bool {
    if !is_responses_path(path) {
        return false;
    }
    if obj.contains_key("instructions") {
        return false;
    }
    // 中文注释：对齐 CPA 的 Codex 请求构造：缺失 instructions 时补空字符串，
    // 避免部分上游对字段存在性更严格导致的 400。
    obj.insert("instructions".to_string(), Value::String(String::new()));
    true
}

pub(super) fn ensure_input_list(path: &str, obj: &mut serde_json::Map<String, Value>) -> bool {
    if !is_responses_path(path) {
        return false;
    }
    let Some(input) = obj.get_mut("input") else {
        return false;
    };
    match input {
        Value::String(text) => {
            *input = Value::Array(vec![build_input_text_message_item(text)]);
            true
        }
        Value::Object(_) => {
            *input = Value::Array(vec![input.clone()]);
            true
        }
        _ => false,
    }
}

fn build_input_text_message_item(text: &str) -> Value {
    let mut content_part = serde_json::Map::new();
    content_part.insert("type".to_string(), Value::String("input_text".to_string()));
    content_part.insert("text".to_string(), Value::String(text.to_string()));

    let mut message_item = serde_json::Map::new();
    message_item.insert("type".to_string(), Value::String("message".to_string()));
    message_item.insert("role".to_string(), Value::String("user".to_string()));
    message_item.insert(
        "content".to_string(),
        Value::Array(vec![Value::Object(content_part)]),
    );
    Value::Object(message_item)
}

fn normalize_input_item(item: &mut Value) -> bool {
    match item {
        Value::String(text) => {
            *item = build_input_text_message_item(text);
            true
        }
        Value::Object(map) => {
            let Some(item_type) = map.get("type").and_then(Value::as_str).map(str::trim) else {
                return false;
            };
            match item_type {
                // 中文注释：对齐 openai/codex 当前协议，客户端历史别名统一改成 wire canonical
                // `compaction`，避免 `/responses/compact` 在数组输入深索引处触发 invalid item type。
                "compact" | "compaction_summary" => {
                    map.insert("type".to_string(), Value::String("compaction".to_string()));
                    true
                }
                _ => false,
            }
        }
        _ => false,
    }
}

pub(super) fn normalize_input_items(
    path: &str,
    obj: &mut serde_json::Map<String, Value>,
) -> bool {
    if !is_responses_path(path) {
        return false;
    }
    let Some(Value::Array(items)) = obj.get_mut("input") else {
        return false;
    };

    let mut changed = false;
    for item in items.iter_mut() {
        if normalize_input_item(item) {
            changed = true;
        }
    }
    changed
}

pub(super) fn ensure_stream_true(path: &str, obj: &mut serde_json::Map<String, Value>) -> bool {
    if !is_standard_responses_path(path) {
        return false;
    }
    let stream = obj
        .entry("stream".to_string())
        .or_insert(Value::Bool(false));
    if stream.as_bool() == Some(true) {
        return false;
    }
    // 中文注释：对齐 CPA 的 Codex executor：/responses 固定走上游 SSE，
    // 后续由网关按下游协议再聚合/透传，避免 backend-api/codex 在非流式形态返回 400。
    *stream = Value::Bool(true);
    true
}

pub(super) fn take_stream_passthrough_flag(
    path: &str,
    obj: &mut serde_json::Map<String, Value>,
) -> bool {
    if !is_responses_path(path) {
        return false;
    }
    obj.remove("stream_passthrough")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub(super) fn ensure_store_false(path: &str, obj: &mut serde_json::Map<String, Value>) -> bool {
    if !is_standard_responses_path(path) {
        return false;
    }
    let store = obj.entry("store".to_string()).or_insert(Value::Bool(false));
    if store.as_bool() == Some(false) {
        return false;
    }
    // 中文注释：Codex upstream 对 /responses 要求 store=false；
    // 用户端若显式传 true，这里统一改写避免上游 400。
    *store = Value::Bool(false);
    true
}

pub(super) fn normalize_dynamic_tools_to_tools(
    path: &str,
    obj: &mut serde_json::Map<String, Value>,
) -> bool {
    if !is_responses_path(path) {
        return false;
    }

    let dynamic_tools = obj
        .remove("dynamic_tools")
        .or_else(|| obj.remove("dynamicTools"));
    let Some(dynamic_tools) = dynamic_tools else {
        return false;
    };
    let dynamic_tools = dynamic_tools.as_array().cloned().unwrap_or_default();

    let tools = obj
        .entry("tools".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !tools.is_array() {
        *tools = Value::Array(Vec::new());
    }
    let Some(tools_array) = tools.as_array_mut() else {
        return true;
    };

    for dynamic_tool in dynamic_tools {
        let Some(tool_obj) = dynamic_tool.as_object() else {
            continue;
        };
        let Some(name) = tool_obj
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };

        let description = tool_obj
            .get("description")
            .cloned()
            .unwrap_or_else(|| Value::String(String::new()));
        let parameters = tool_obj
            .get("input_schema")
            .or_else(|| tool_obj.get("inputSchema"))
            .or_else(|| tool_obj.get("parameters"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} }));

        let mut mapped = serde_json::Map::new();
        mapped.insert("type".to_string(), Value::String("function".to_string()));
        mapped.insert("name".to_string(), Value::String(name.to_string()));
        mapped.insert("description".to_string(), description);
        mapped.insert("parameters".to_string(), parameters);
        if let Some(strict) = tool_obj.get("strict") {
            mapped.insert("strict".to_string(), strict.clone());
        }
        tools_array.push(Value::Object(mapped));
    }

    true
}

pub(super) fn apply_reasoning_override(
    path: &str,
    obj: &mut serde_json::Map<String, Value>,
    reasoning_effort: Option<&str>,
) -> bool {
    if !is_responses_path(path) {
        return false;
    }
    let Some(level) = reasoning_effort else {
        return false;
    };
    let reasoning = obj
        .entry("reasoning".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !reasoning.is_object() {
        // 中文注释：某些客户端会把 reasoning 误传成字符串；不矫正为对象会导致 effort 覆盖失效。
        *reasoning = Value::Object(serde_json::Map::new());
    }
    if let Some(reasoning_obj) = reasoning.as_object_mut() {
        reasoning_obj.insert("effort".to_string(), Value::String(level.to_string()));
        return true;
    }
    false
}

fn is_supported_openai_responses_key(key: &str) -> bool {
    matches!(
        key,
        "include"
            | "input"
            | "instructions"
            | "max_output_tokens"
            | "metadata"
            | "model"
            | "parallel_tool_calls"
            | "previous_response_id"
            | "reasoning"
            | "service_tier"
            | "store"
            | "stream"
            | "temperature"
            | "text"
            | "tool_choice"
            | "tools"
            | "top_p"
            | "truncation"
            | "user"
            | "stream_passthrough"
    )
}

fn is_supported_openai_compact_key(key: &str) -> bool {
    matches!(
        key,
        "input"
            | "instructions"
            | "metadata"
            | "model"
            | "parallel_tool_calls"
            | "reasoning"
            | "text"
            | "tools"
    )
}

pub(super) fn retain_official_fields(
    path: &str,
    obj: &mut serde_json::Map<String, Value>,
) -> Vec<String> {
    retain_fields_by_templates(
        path,
        obj,
        &[
            TemplateAllowlist {
                template: "/v1/responses/compact",
                allow: is_supported_openai_compact_key,
            },
            TemplateAllowlist {
                template: "/v1/responses",
                allow: is_supported_openai_responses_key,
            },
        ],
    )
}

fn is_supported_codex_responses_key(key: &str) -> bool {
    matches!(
        key,
        "model"
            | "instructions"
            | "input"
            | "tools"
            | "tool_choice"
            | "parallel_tool_calls"
            | "reasoning"
            | "service_tier"
            | "store"
            | "stream"
            | "include"
            | "prompt_cache_key"
            | "encrypted_content"
            | "text"
    )
}

fn is_supported_codex_compact_key(key: &str) -> bool {
    matches!(
        key,
        "model" | "instructions" | "input" | "tools" | "parallel_tool_calls" | "reasoning" | "text"
    )
}

pub(super) fn retain_codex_fields(
    path: &str,
    obj: &mut serde_json::Map<String, Value>,
) -> Vec<String> {
    // 中文注释：仅保留 Codex CLI 固定字段集合，其他字段全部丢弃。
    // `/responses/compact` 与普通 `/responses` 的 wire shape 不同：
    // 前者是非流式 JSON compaction 请求，不接受 `stream` / `store` / `service_tier` 等字段。
    retain_fields_by_templates(
        path,
        obj,
        &[
            TemplateAllowlist {
                template: "/v1/responses/compact",
                allow: is_supported_codex_compact_key,
            },
            TemplateAllowlist {
                template: "/v1/responses",
                allow: is_supported_codex_responses_key,
            },
        ],
    )
}
