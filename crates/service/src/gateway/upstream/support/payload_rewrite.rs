use serde_json::Value;

pub(in super::super) fn body_has_encrypted_content_hint(body: &[u8]) -> bool {
    // Fast path: avoid JSON parsing unless we hit a recovery path.
    std::str::from_utf8(body)
        .ok()
        .is_some_and(|text| text.contains("\"encrypted_content\""))
}

fn strip_encrypted_content_value(value: &mut Value) -> bool {
    match value {
        Value::Object(map) => {
            let mut changed = map.remove("encrypted_content").is_some();
            for child in map.values_mut() {
                if strip_encrypted_content_value(child) {
                    changed = true;
                }
            }
            changed
        }
        Value::Array(items) => {
            let mut changed = false;
            for item in items.iter_mut() {
                if strip_encrypted_content_value(item) {
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
    if !strip_encrypted_content_value(&mut value) {
        return None;
    }
    serde_json::to_vec(&value).ok()
}
