use codexmanager_core::storage::{now_ts, Event};

use crate::{account_status, storage_helpers::open_storage};

pub(crate) fn update_account(
    account_id: &str,
    sort: Option<i64>,
    status: Option<&str>,
    label: Option<&str>,
    note: Option<&str>,
    tags: Option<&str>,
    group_name: Option<&str>,
) -> Result<(), String> {
    // 更新账号排序或状态并记录事件
    let normalized_account_id = account_id.trim();
    if normalized_account_id.is_empty() {
        return Err("missing accountId".to_string());
    }

    let normalized_status = status.map(normalize_account_status).transpose()?;
    let normalized_label = normalize_optional_label(label)?;
    let normalized_note = normalize_optional_text(note);
    let normalized_tags = normalize_optional_tags(tags);
    let normalized_group_name = normalize_optional_group_name(group_name);
    let metadata_requested = note.is_some() || tags.is_some();
    let group_name_requested = group_name.is_some();

    if sort.is_none()
        && normalized_status.is_none()
        && normalized_label.is_none()
        && !metadata_requested
        && !group_name_requested
    {
        return Err("missing account update fields".to_string());
    }

    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    let now = now_ts();
    if let Some(sort) = sort {
        storage
            .update_account_sort(normalized_account_id, sort)
            .map_err(|e| e.to_string())?;
        let _ = storage.insert_event(&Event {
            account_id: Some(normalized_account_id.to_string()),
            event_type: "account_sort_update".to_string(),
            message: format!("sort={sort}"),
            created_at: now,
        });
    }

    if let Some(status) = normalized_status {
        let reason = if status == "disabled" {
            "manual_disable"
        } else {
            "manual_enable"
        };
        account_status::set_account_status(&storage, normalized_account_id, status, reason);
    }

    if let Some(label) = normalized_label {
        storage
            .update_account_label(normalized_account_id, label)
            .map_err(|e| e.to_string())?;
        let _ = storage.insert_event(&Event {
            account_id: Some(normalized_account_id.to_string()),
            event_type: "account_profile_update".to_string(),
            message: format!("label={label}"),
            created_at: now,
        });
    }

    if group_name_requested {
        storage
            .update_account_group_name(normalized_account_id, normalized_group_name.as_deref())
            .map_err(|e| e.to_string())?;
        let _ = storage.insert_event(&Event {
            account_id: Some(normalized_account_id.to_string()),
            event_type: "account_profile_update".to_string(),
            message: format!(
                "group_name={}",
                normalized_group_name.as_deref().unwrap_or("-")
            ),
            created_at: now,
        });
    }

    if metadata_requested {
        storage
            .upsert_account_metadata(
                normalized_account_id,
                normalized_note.as_deref(),
                normalized_tags.as_deref(),
            )
            .map_err(|e| e.to_string())?;
        storage
            .touch_account_updated_at(normalized_account_id)
            .map_err(|e| e.to_string())?;
        let _ = storage.insert_event(&Event {
            account_id: Some(normalized_account_id.to_string()),
            event_type: "account_profile_update".to_string(),
            message: format!(
                "note={} tags={}",
                normalized_note.as_deref().unwrap_or("-"),
                normalized_tags.as_deref().unwrap_or("-"),
            ),
            created_at: now,
        });
    }

    Ok(())
}

fn normalize_account_status(status: &str) -> Result<&'static str, String> {
    let normalized = status.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "active" => Ok("active"),
        "disabled" | "inactive" => Ok("disabled"),
        _ => Err(format!("unsupported account status: {status}")),
    }
}

fn normalize_optional_label(label: Option<&str>) -> Result<Option<&str>, String> {
    let Some(label) = label else {
        return Ok(None);
    };
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return Err("label cannot be empty".to_string());
    }
    Ok(Some(trimmed))
}

fn normalize_optional_text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn normalize_optional_tags(value: Option<&str>) -> Option<String> {
    let Some(value) = value else {
        return None;
    };
    let parts = value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(","))
    }
}

fn normalize_optional_group_name(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|group_name| !group_name.is_empty())
        .map(ToString::to_string)
}
