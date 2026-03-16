use codexmanager_core::{
    auth::parse_id_token_claims,
    storage::{Storage, Token, UsageSnapshotRecord},
};
use serde_json::Value;

const MINUTES_PER_HOUR: i64 = 60;
const MINUTES_PER_DAY: i64 = 24 * MINUTES_PER_HOUR;
const ROUNDING_BIAS: i64 = 3;

pub(crate) fn extract_plan_type_from_id_token(id_token: &str) -> Option<String> {
    parse_id_token_claims(id_token)
        .ok()
        .and_then(|claims| claims.auth)
        .and_then(|auth| auth.chatgpt_plan_type)
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

pub(crate) fn is_free_plan_type(plan_type: Option<&str>) -> bool {
    let Some(plan_type) = plan_type else {
        return false;
    };
    let normalized = plan_type.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    normalized.contains("free")
}

pub(crate) fn is_free_plan_from_credits_json(raw_credits_json: Option<&str>) -> bool {
    let Some(raw_credits_json) = raw_credits_json else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(raw_credits_json) else {
        return false;
    };
    let keys = [
        "plan_type",
        "planType",
        "subscription_tier",
        "subscriptionTier",
        "tier",
        "account_type",
        "accountType",
        "type",
    ];
    let extracted = extract_string_by_keys_recursive(&value, &keys);
    is_free_plan_type(extracted.as_deref())
}

pub(crate) fn is_single_window_long_usage_snapshot(snapshot: &UsageSnapshotRecord) -> bool {
    let has_primary_signal = snapshot.used_percent.is_some() || snapshot.window_minutes.is_some();
    let has_secondary_signal =
        snapshot.secondary_used_percent.is_some() || snapshot.secondary_window_minutes.is_some();
    has_primary_signal && !has_secondary_signal && is_long_window(snapshot.window_minutes)
}

pub(crate) fn is_free_or_single_window_account(
    storage: &Storage,
    account_id: &str,
    token: &Token,
) -> bool {
    if is_free_plan_type(extract_plan_type_from_id_token(&token.id_token).as_deref())
        || is_free_plan_type(extract_plan_type_from_id_token(&token.access_token).as_deref())
    {
        return true;
    }

    storage
        .latest_usage_snapshot_for_account(account_id)
        .ok()
        .flatten()
        .map(|snapshot| {
            is_free_plan_from_credits_json(snapshot.credits_json.as_deref())
                || is_single_window_long_usage_snapshot(&snapshot)
        })
        .unwrap_or(false)
}

fn is_long_window(window_minutes: Option<i64>) -> bool {
    window_minutes.is_some_and(|value| value > MINUTES_PER_DAY + ROUNDING_BIAS)
}

fn extract_string_by_keys_recursive(value: &Value, keys: &[&str]) -> Option<String> {
    if let Some(object) = value.as_object() {
        for key in keys {
            let candidate = object
                .get(*key)
                .and_then(Value::as_str)
                .map(|text| text.trim().to_ascii_lowercase())
                .filter(|text| !text.is_empty());
            if candidate.is_some() {
                return candidate;
            }
        }
        for child in object.values() {
            let nested = extract_string_by_keys_recursive(child, keys);
            if nested.is_some() {
                return nested;
            }
        }
        return None;
    }
    if let Some(array) = value.as_array() {
        for child in array {
            let nested = extract_string_by_keys_recursive(child, keys);
            if nested.is_some() {
                return nested;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        extract_plan_type_from_id_token, is_free_or_single_window_account,
        is_free_plan_from_credits_json, is_free_plan_type, is_single_window_long_usage_snapshot,
    };
    use codexmanager_core::storage::{now_ts, Account, Storage, Token, UsageSnapshotRecord};

    fn encode_base64url(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        let mut index = 0;
        while index + 3 <= bytes.len() {
            let chunk = ((bytes[index] as u32) << 16)
                | ((bytes[index + 1] as u32) << 8)
                | (bytes[index + 2] as u32);
            out.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
            out.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
            out.push(TABLE[((chunk >> 6) & 0x3f) as usize] as char);
            out.push(TABLE[(chunk & 0x3f) as usize] as char);
            index += 3;
        }
        match bytes.len().saturating_sub(index) {
            1 => {
                let chunk = (bytes[index] as u32) << 16;
                out.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
                out.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
            }
            2 => {
                let chunk = ((bytes[index] as u32) << 16) | ((bytes[index + 1] as u32) << 8);
                out.push(TABLE[((chunk >> 18) & 0x3f) as usize] as char);
                out.push(TABLE[((chunk >> 12) & 0x3f) as usize] as char);
                out.push(TABLE[((chunk >> 6) & 0x3f) as usize] as char);
            }
            _ => {}
        }
        out
    }

    #[test]
    fn free_plan_detection_accepts_common_variants() {
        assert!(is_free_plan_type(Some("free")));
        assert!(is_free_plan_type(Some("ChatGPT_Free")));
        assert!(is_free_plan_type(Some("free_tier")));
    }

    #[test]
    fn free_plan_detection_rejects_paid_or_unknown_variants() {
        assert!(!is_free_plan_type(None));
        assert!(!is_free_plan_type(Some("")));
        assert!(!is_free_plan_type(Some("plus")));
        assert!(!is_free_plan_type(Some("pro")));
        assert!(!is_free_plan_type(Some("team")));
    }

    #[test]
    fn free_plan_detection_accepts_credits_json_marker() {
        let credits_json = r#"{"planType":"free"}"#;
        assert!(is_free_plan_from_credits_json(Some(credits_json)));
    }

    #[test]
    fn extract_plan_type_from_id_token_reads_chatgpt_claim() {
        let header = encode_base64url(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = encode_base64url(
            serde_json::json!({
                "sub": "acc-plan-free",
                "https://api.openai.com/auth": {
                    "chatgpt_plan_type": "free"
                }
            })
            .to_string()
            .as_bytes(),
        );
        let token = format!("{header}.{payload}.sig");
        assert_eq!(
            extract_plan_type_from_id_token(&token).as_deref(),
            Some("free")
        );
    }

    #[test]
    fn single_window_long_usage_snapshot_counts_as_free_like() {
        let snapshot = UsageSnapshotRecord {
            account_id: "acc-free".to_string(),
            used_percent: Some(20.0),
            window_minutes: Some(10_080),
            resets_at: None,
            secondary_used_percent: None,
            secondary_window_minutes: None,
            secondary_resets_at: None,
            credits_json: None,
            captured_at: now_ts(),
        };

        assert!(is_single_window_long_usage_snapshot(&snapshot));
    }

    #[test]
    fn free_or_single_window_account_accepts_weekly_single_window_without_plan_claim() {
        let storage = Storage::open_in_memory().expect("open");
        storage.init().expect("init");
        let now = now_ts();
        storage
            .insert_account(&Account {
                id: "acc-weekly".to_string(),
                label: "acc-weekly".to_string(),
                issuer: "issuer".to_string(),
                chatgpt_account_id: None,
                workspace_id: None,
                group_name: None,
                sort: 0,
                status: "active".to_string(),
                created_at: now,
                updated_at: now,
            })
            .expect("insert account");
        let token = Token {
            account_id: "acc-weekly".to_string(),
            id_token: "header.payload.sig".to_string(),
            access_token: "header.payload.sig".to_string(),
            refresh_token: "refresh".to_string(),
            api_key_access_token: None,
            last_refresh: now,
        };
        storage.insert_token(&token).expect("insert token");
        storage
            .insert_usage_snapshot(&UsageSnapshotRecord {
                account_id: "acc-weekly".to_string(),
                used_percent: Some(25.0),
                window_minutes: Some(10_080),
                resets_at: None,
                secondary_used_percent: None,
                secondary_window_minutes: None,
                secondary_resets_at: None,
                credits_json: None,
                captured_at: now,
            })
            .expect("insert usage");

        assert!(is_free_or_single_window_account(
            &storage,
            "acc-weekly",
            &token
        ));
    }
}
