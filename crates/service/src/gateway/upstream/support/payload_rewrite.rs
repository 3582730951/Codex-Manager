use serde_json::Value;

pub(in super::super) fn body_has_encrypted_content_hint(body: &[u8]) -> bool {
    // Fast path: avoid JSON parsing unless we hit a recovery path.
    std::str::from_utf8(body)
        .ok()
        .is_some_and(|text| text.contains("\"encrypted_content\""))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StripResult {
    Unchanged,
    Changed,
    RemoveSelf,
}

fn strip_encrypted_content_value(value: &mut Value) -> StripResult {
    match value {
        Value::Object(map) => {
            let is_compaction_item = map
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("compaction"));
            let mut changed = map.remove("encrypted_content").is_some();
            let child_keys = map.keys().cloned().collect::<Vec<_>>();
            for key in child_keys {
                let mut remove_child = false;
                if let Some(child) = map.get_mut(&key) {
                    match strip_encrypted_content_value(child) {
                        StripResult::Unchanged => {}
                        StripResult::Changed => {
                            changed = true;
                        }
                        StripResult::RemoveSelf => {
                            changed = true;
                            remove_child = true;
                        }
                    }
                }
                if remove_child {
                    map.remove(&key);
                }
            }

            // 中文注释：`compaction` item 的核心载荷就是 `encrypted_content`。
            // 当会话锚点被收敛/重置时，保留一个裸 `type=compaction` item
            // 更容易触发 upstream `input[n] compat` 校验错误，因此整项移除。
            if is_compaction_item {
                return StripResult::RemoveSelf;
            }

            if changed {
                StripResult::Changed
            } else {
                StripResult::Unchanged
            }
        }
        Value::Array(items) => {
            let mut changed = false;
            let mut retained = Vec::with_capacity(items.len());
            for mut item in std::mem::take(items) {
                match strip_encrypted_content_value(&mut item) {
                    StripResult::Unchanged => retained.push(item),
                    StripResult::Changed => {
                        changed = true;
                        retained.push(item);
                    }
                    StripResult::RemoveSelf => {
                        changed = true;
                    }
                }
            }
            *items = retained;
            if changed {
                StripResult::Changed
            } else {
                StripResult::Unchanged
            }
        }
        _ => StripResult::Unchanged,
    }
}

pub(in super::super) fn strip_encrypted_content_from_body(body: &[u8]) -> Option<Vec<u8>> {
    let mut value: Value = serde_json::from_slice(body).ok()?;
    match strip_encrypted_content_value(&mut value) {
        StripResult::Unchanged | StripResult::RemoveSelf => None,
        StripResult::Changed => serde_json::to_vec(&value).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::{body_has_encrypted_content_hint, strip_encrypted_content_from_body};
    use serde_json::json;

    #[test]
    fn strip_encrypted_content_removes_compaction_items_from_input_history() {
        let body = json!({
            "model": "gpt-5.3-codex",
            "prompt_cache_key": "cache_anchor_123",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "hello" }]
                },
                {
                    "type": "compaction",
                    "encrypted_content": "gAAA_compaction_blob"
                }
            ]
        });
        let body = serde_json::to_vec(&body).expect("serialize body");
        assert!(body_has_encrypted_content_hint(&body));

        let stripped = strip_encrypted_content_from_body(&body).expect("body should be rewritten");
        let value: serde_json::Value =
            serde_json::from_slice(&stripped).expect("parse stripped body");
        let input = value["input"].as_array().expect("input should be array");
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(value["prompt_cache_key"], "cache_anchor_123");
        assert!(!String::from_utf8_lossy(&stripped).contains("\"encrypted_content\""));
    }

    #[test]
    fn strip_encrypted_content_keeps_reasoning_summary_blocks() {
        let body = json!({
            "input": [
                {
                    "type": "reasoning",
                    "summary": [{ "type": "summary_text", "text": "keep me" }],
                    "encrypted_content": "gAAA_reasoning_blob"
                }
            ]
        });
        let stripped = strip_encrypted_content_from_body(
            &serde_json::to_vec(&body).expect("serialize reasoning body"),
        )
        .expect("reasoning body should be rewritten");
        let value: serde_json::Value =
            serde_json::from_slice(&stripped).expect("parse stripped reasoning body");
        assert_eq!(value["input"][0]["type"], "reasoning");
        assert_eq!(value["input"][0]["summary"][0]["text"], "keep me");
        assert!(value["input"][0].get("encrypted_content").is_none());
    }
}
