use serde_json::Value;

pub(in super::super) fn body_has_encrypted_content_hint(body: &[u8]) -> bool {
    // Fast path: avoid JSON parsing unless we hit a recovery path.
    std::str::from_utf8(body)
        .ok()
        .is_some_and(|text| text.contains("\"encrypted_content\""))
}

fn strip_encrypted_content_value(value: &mut Value, preserve_input_items: bool) -> bool {
    match value {
        Value::Object(map) => {
            // 中文注释：CLI continuation / compaction 历史会把必填的
            // `encrypted_content` 放进 `input[*]` 项里；这些字段不能在 session strip 时递归删掉，
            // 否则上游会直接返回 `Missing required parameter: input[n].encrypted_content`。
            let mut changed = if preserve_input_items {
                false
            } else {
                map.remove("encrypted_content").is_some()
            };
            for (key, child) in map.iter_mut() {
                if strip_encrypted_content_value(child, preserve_input_items || key == "input") {
                    changed = true;
                }
            }
            changed
        }
        Value::Array(items) => {
            let mut changed = false;
            for item in items.iter_mut() {
                if strip_encrypted_content_value(item, preserve_input_items) {
                    changed = true;
                }
            }
            changed
        }
        _ => false,
    }
}

pub(in super::super) fn strip_encrypted_content_from_body(body: &[u8]) -> Option<Vec<u8>> {
    let mut value: Value = serde_json::from_slice(body).ok()?;
    if !strip_encrypted_content_value(&mut value, false) {
        return None;
    }
    serde_json::to_vec(&value).ok()
}

#[cfg(test)]
mod tests {
    use super::strip_encrypted_content_from_body;

    #[test]
    fn strip_encrypted_content_keeps_required_input_item_blobs() {
        let body = serde_json::json!({
            "model": "gpt-5",
            "encrypted_content": "drop-top-level",
            "metadata": {
                "encrypted_content": "drop-metadata"
            },
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "hello" }]
                },
                {
                    "type": "compaction",
                    "encrypted_content": "keep-compaction"
                },
                {
                    "type": "reasoning",
                    "encrypted_content": "keep-reasoning",
                    "summary": [{ "type": "summary_text", "text": "reuse context" }]
                }
            ]
        });

        let actual = strip_encrypted_content_from_body(
            serde_json::to_vec(&body).expect("serialize").as_slice(),
        )
        .expect("rewritten body");
        let value: serde_json::Value =
            serde_json::from_slice(actual.as_slice()).expect("parse rewritten");

        assert!(value.get("encrypted_content").is_none());
        assert!(value["metadata"].get("encrypted_content").is_none());
        assert_eq!(value["input"][1]["encrypted_content"], "keep-compaction");
        assert_eq!(value["input"][2]["encrypted_content"], "keep-reasoning");
    }
}
