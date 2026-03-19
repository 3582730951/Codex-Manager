#[allow(unused_imports)]
use super::{adapt_request_for_protocol, ResponseAdapter};
use crate::apikey_profile::PROTOCOL_ANTHROPIC_NATIVE;

#[test]
fn anthropic_messages_are_the_only_path_adapted_to_responses() {
    let body =
        br#"{"model":"claude-3-5-sonnet","messages":[{"role":"user","content":"hello"}]}"#.to_vec();
    let adapted = adapt_request_for_protocol(PROTOCOL_ANTHROPIC_NATIVE, "/v1/messages", body)
        .expect("adapt request");
    assert_eq!(adapted.path, "/v1/responses");
    assert_ne!(adapted.response_adapter, ResponseAdapter::Passthrough);
}

#[test]
fn anthropic_messages_map_text_and_base64_image_to_responses_input() {
    let body = serde_json::json!({
        "model": "claude-3-5-sonnet",
        "messages": [{
            "role": "user",
            "content": [
                {
                    "type": "text",
                    "text": "阅读一下这个图片里面是什么"
                },
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": "ZmFrZV9pbWFnZQ=="
                    }
                }
            ]
        }]
    });
    let adapted = adapt_request_for_protocol(
        PROTOCOL_ANTHROPIC_NATIVE,
        "/v1/messages",
        serde_json::to_vec(&body).expect("serialize body"),
    )
    .expect("adapt request");
    let value: serde_json::Value =
        serde_json::from_slice(&adapted.body).expect("parse adapted body");
    assert_eq!(adapted.path, "/v1/responses");
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(0))
            .and_then(|item| item.get("role"))
            .and_then(serde_json::Value::as_str),
        Some("user")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(0))
            .and_then(|item| item.get("content"))
            .and_then(|content| content.get(0))
            .and_then(|part| part.get("type"))
            .and_then(serde_json::Value::as_str),
        Some("input_text")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(0))
            .and_then(|item| item.get("content"))
            .and_then(|content| content.get(0))
            .and_then(|part| part.get("text"))
            .and_then(serde_json::Value::as_str),
        Some("阅读一下这个图片里面是什么")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(0))
            .and_then(|item| item.get("content"))
            .and_then(|content| content.get(1))
            .and_then(|part| part.get("type"))
            .and_then(serde_json::Value::as_str),
        Some("input_image")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(0))
            .and_then(|item| item.get("content"))
            .and_then(|content| content.get(1))
            .and_then(|part| part.get("image_url"))
            .and_then(serde_json::Value::as_str),
        Some("data:image/png;base64,ZmFrZV9pbWFnZQ==")
    );
}

#[test]
fn anthropic_messages_map_url_image_to_responses_input() {
    let body = serde_json::json!({
        "model": "claude-3-5-sonnet",
        "messages": [{
            "role": "user",
            "content": [{
                "type": "image",
                "source": {
                    "type": "url",
                    "url": "https://example.com/screenshot.png"
                }
            }]
        }]
    });
    let adapted = adapt_request_for_protocol(
        PROTOCOL_ANTHROPIC_NATIVE,
        "/v1/messages",
        serde_json::to_vec(&body).expect("serialize body"),
    )
    .expect("adapt request");
    let value: serde_json::Value =
        serde_json::from_slice(&adapted.body).expect("parse adapted body");
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(0))
            .and_then(|item| item.get("content"))
            .and_then(|content| content.get(0))
            .and_then(|part| part.get("image_url"))
            .and_then(serde_json::Value::as_str),
        Some("https://example.com/screenshot.png")
    );
}

#[test]
fn anthropic_messages_map_thinking_and_output_config_effort_to_responses() {
    let body = serde_json::json!({
        "model": "claude-sonnet-4-5",
        "thinking": {
            "type": "enabled",
            "budget_tokens": 10000
        },
        "output_config": {
            "effort": "medium"
        },
        "messages": [
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "thinking",
                        "thinking": "先检查上下文。",
                        "signature": "sig_claude_prev_turn"
                    }
                ]
            },
            {
                "role": "user",
                "content": "继续"
            }
        ]
    });
    let adapted = adapt_request_for_protocol(
        PROTOCOL_ANTHROPIC_NATIVE,
        "/v1/messages",
        serde_json::to_vec(&body).expect("serialize body"),
    )
    .expect("adapt request");
    let value: serde_json::Value =
        serde_json::from_slice(&adapted.body).expect("parse adapted body");

    assert_eq!(
        value
            .get("reasoning")
            .and_then(|reasoning| reasoning.get("effort"))
            .and_then(serde_json::Value::as_str),
        Some("medium")
    );
    assert_eq!(
        value
            .get("reasoning")
            .and_then(|reasoning| reasoning.get("summary"))
            .and_then(serde_json::Value::as_str),
        Some("detailed")
    );
    assert_eq!(
        value
            .get("encrypted_content")
            .and_then(serde_json::Value::as_str),
        Some("sig_claude_prev_turn")
    );
}

#[test]
fn anthropic_messages_map_disabled_thinking_to_summary_none() {
    let body = serde_json::json!({
        "model": "claude-sonnet-4-5",
        "thinking": {
            "type": "disabled"
        },
        "messages": [{
            "role": "user",
            "content": "hello"
        }]
    });
    let adapted = adapt_request_for_protocol(
        PROTOCOL_ANTHROPIC_NATIVE,
        "/v1/messages",
        serde_json::to_vec(&body).expect("serialize body"),
    )
    .expect("adapt request");
    let value: serde_json::Value =
        serde_json::from_slice(&adapted.body).expect("parse adapted body");

    assert_eq!(
        value
            .get("reasoning")
            .and_then(|reasoning| reasoning.get("summary"))
            .and_then(serde_json::Value::as_str),
        Some("none")
    );
}

#[test]
fn anthropic_assistant_tool_use_preserves_text_order_in_responses_input() {
    let body = serde_json::json!({
        "model": "claude-3-5-sonnet",
        "messages": [
            {
                "role": "user",
                "content": "继续上一轮"
            },
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "text",
                        "text": "先读取 README。"
                    },
                    {
                        "type": "tool_use",
                        "id": "toolu_readme_1",
                        "name": "read_file",
                        "input": {
                            "path": "README.md"
                        }
                    },
                    {
                        "type": "text",
                        "text": "读取后继续总结。"
                    }
                ]
            }
        ]
    });
    let adapted = adapt_request_for_protocol(
        PROTOCOL_ANTHROPIC_NATIVE,
        "/v1/messages",
        serde_json::to_vec(&body).expect("serialize body"),
    )
    .expect("adapt request");
    let value: serde_json::Value =
        serde_json::from_slice(&adapted.body).expect("parse adapted body");

    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(1))
            .and_then(|item| item.get("type"))
            .and_then(serde_json::Value::as_str),
        Some("message")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(1))
            .and_then(|item| item.get("role"))
            .and_then(serde_json::Value::as_str),
        Some("assistant")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(1))
            .and_then(|item| item.get("content"))
            .and_then(|content| content.get(0))
            .and_then(|part| part.get("text"))
            .and_then(serde_json::Value::as_str),
        Some("先读取 README。")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(2))
            .and_then(|item| item.get("type"))
            .and_then(serde_json::Value::as_str),
        Some("function_call")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(2))
            .and_then(|item| item.get("call_id"))
            .and_then(serde_json::Value::as_str),
        Some("toolu_readme_1")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(2))
            .and_then(|item| item.get("name"))
            .and_then(serde_json::Value::as_str),
        Some("read_file")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(2))
            .and_then(|item| item.get("arguments"))
            .and_then(serde_json::Value::as_str),
        Some("{\"path\":\"README.md\"}")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(3))
            .and_then(|item| item.get("type"))
            .and_then(serde_json::Value::as_str),
        Some("message")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(3))
            .and_then(|item| item.get("role"))
            .and_then(serde_json::Value::as_str),
        Some("assistant")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(3))
            .and_then(|item| item.get("content"))
            .and_then(|content| content.get(0))
            .and_then(|part| part.get("text"))
            .and_then(serde_json::Value::as_str),
        Some("读取后继续总结。")
    );
}

#[test]
fn anthropic_tool_result_with_image_maps_to_function_call_output_items() {
    let body = serde_json::json!({
        "model": "claude-3-5-sonnet",
        "messages": [
            {
                "role": "assistant",
                "content": [
                    {
                        "type": "tool_use",
                        "id": "toolu_image_1",
                        "name": "inspect_image",
                        "input": {
                            "path": "screen.png"
                        }
                    }
                ]
            },
            {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "toolu_image_1",
                        "content": [
                            {
                                "type": "text",
                                "text": "这是截图。"
                            },
                            {
                                "type": "image",
                                "source": {
                                    "type": "base64",
                                    "media_type": "image/png",
                                    "data": "ZmFrZV9pbWFnZQ=="
                                }
                            }
                        ]
                    }
                ]
            }
        ]
    });
    let adapted = adapt_request_for_protocol(
        PROTOCOL_ANTHROPIC_NATIVE,
        "/v1/messages",
        serde_json::to_vec(&body).expect("serialize body"),
    )
    .expect("adapt request");
    let value: serde_json::Value =
        serde_json::from_slice(&adapted.body).expect("parse adapted body");

    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(1))
            .and_then(|item| item.get("type"))
            .and_then(serde_json::Value::as_str),
        Some("function_call_output")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(1))
            .and_then(|item| item.get("call_id"))
            .and_then(serde_json::Value::as_str),
        Some("toolu_image_1")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(1))
            .and_then(|item| item.get("output"))
            .and_then(|output| output.get(0))
            .and_then(|part| part.get("type"))
            .and_then(serde_json::Value::as_str),
        Some("input_text")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(1))
            .and_then(|item| item.get("output"))
            .and_then(|output| output.get(0))
            .and_then(|part| part.get("text"))
            .and_then(serde_json::Value::as_str),
        Some("这是截图。")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(1))
            .and_then(|item| item.get("output"))
            .and_then(|output| output.get(1))
            .and_then(|part| part.get("type"))
            .and_then(serde_json::Value::as_str),
        Some("input_image")
    );
    assert_eq!(
        value
            .get("input")
            .and_then(|input| input.get(1))
            .and_then(|item| item.get("output"))
            .and_then(|output| output.get(1))
            .and_then(|part| part.get("image_url"))
            .and_then(serde_json::Value::as_str),
        Some("data:image/png;base64,ZmFrZV9pbWFnZQ==")
    );
}

#[test]
fn anthropic_messages_preserve_all_tools_across_multiple_mcp_servers() {
    let mut tools = Vec::new();
    for index in 0..16 {
        tools.push(serde_json::json!({
            "name": format!("mcp__server_alpha__tool_{index:02}"),
            "description": "alpha",
            "input_schema": { "type": "object", "properties": {} }
        }));
    }
    tools.push(serde_json::json!({
        "name": "mcp__server_beta__lookup",
        "description": "beta lookup",
        "input_schema": { "type": "object", "properties": {} }
    }));
    tools.push(serde_json::json!({
        "name": "mcp__server_beta__fetch",
        "description": "beta fetch",
        "input_schema": { "type": "object", "properties": {} }
    }));

    let body = serde_json::json!({
        "model": "claude-sonnet-4-5",
        "messages": [{ "role": "user", "content": "同时使用两个 MCP server" }],
        "tools": tools
    });
    let adapted = adapt_request_for_protocol(
        PROTOCOL_ANTHROPIC_NATIVE,
        "/v1/messages",
        serde_json::to_vec(&body).expect("serialize body"),
    )
    .expect("adapt request");
    let value: serde_json::Value =
        serde_json::from_slice(&adapted.body).expect("parse adapted body");
    let mapped_tools = value
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .expect("tools array");

    assert_eq!(mapped_tools.len(), 18);
    assert!(mapped_tools.iter().any(|tool| {
        tool.get("name").and_then(serde_json::Value::as_str) == Some("mcp__server_beta__lookup")
    }));
    assert!(mapped_tools.iter().any(|tool| {
        tool.get("name").and_then(serde_json::Value::as_str) == Some("mcp__server_beta__fetch")
    }));
}

#[test]
fn anthropic_messages_shorten_long_tool_names_and_build_restore_map() {
    let original_tool_name =
        "mcp__plugin_super_long_workspace_namespace__tool_server_namespace_for_codex_manager_gateway_adapter_alignment__very_long_tool_operation_name";
    let body = serde_json::json!({
        "model": "claude-sonnet-4-5",
        "messages": [
            {
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_long_1",
                    "name": original_tool_name,
                    "input": { "path": "README.md" }
                }]
            },
            {
                "role": "user",
                "content": "继续"
            }
        ],
        "tools": [{
            "name": original_tool_name,
            "description": "long tool",
            "input_schema": { "type": "object", "properties": {} }
        }],
        "tool_choice": {
            "type": "tool",
            "name": original_tool_name
        }
    });
    let adapted = adapt_request_for_protocol(
        PROTOCOL_ANTHROPIC_NATIVE,
        "/v1/messages",
        serde_json::to_vec(&body).expect("serialize body"),
    )
    .expect("adapt request");
    let value: serde_json::Value =
        serde_json::from_slice(&adapted.body).expect("parse adapted body");
    let shortened_name = value
        .get("tools")
        .and_then(|tools| tools.get(0))
        .and_then(|tool| tool.get("name"))
        .and_then(serde_json::Value::as_str)
        .expect("tools[0].name")
        .to_string();

    assert_ne!(shortened_name, original_tool_name);
    assert!(shortened_name.len() <= 64);
    assert_eq!(
        adapted.tool_name_restore_map.get(&shortened_name),
        Some(&original_tool_name.to_string())
    );
    assert_eq!(
        value
            .get("tool_choice")
            .and_then(|tool_choice| tool_choice.get("name"))
            .and_then(serde_json::Value::as_str),
        Some(shortened_name.as_str())
    );
    assert_eq!(
        value
            .get("input")
            .and_then(serde_json::Value::as_array)
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("type").and_then(serde_json::Value::as_str) == Some("function_call")
                })
            })
            .and_then(|item| item.get("name"))
            .and_then(serde_json::Value::as_str),
        Some(shortened_name.as_str())
    );
}
