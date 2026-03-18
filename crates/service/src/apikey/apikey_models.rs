use codexmanager_core::rpc::types::ApiKeyModelListResult;
use codexmanager_core::rpc::types::ModelOption;
use codexmanager_core::storage::now_ts;
use std::collections::BTreeMap;

use crate::gateway;
use crate::storage_helpers;

const MODEL_CACHE_SCOPE_DEFAULT: &str = "default";
const DEFAULT_MODEL_SLUGS: &[&str] = &[
    "gpt-5",
    "gpt-5-codex",
    "gpt-5-codex-mini",
    "gpt-5.1",
    "gpt-5.1-codex",
    "gpt-5.1-codex-max",
    "gpt-5.1-codex-mini",
    "gpt-5.2",
    "gpt-5.2-codex",
    "gpt-5.3-codex",
    "gpt-5.4",
];

pub(crate) fn read_model_options(refresh_remote: bool) -> Result<ApiKeyModelListResult, String> {
    if !refresh_remote {
        let items = read_cached_model_options_merged()?;
        return Ok(ApiKeyModelListResult { items });
    }

    match gateway::fetch_models_for_picker() {
        Ok(items) => {
            let items = merge_model_options(items, read_local_configured_model_options()?);
            let _ = save_model_options_cache(&items);
            Ok(ApiKeyModelListResult { items })
        }
        Err(err) => {
            let cached = read_cached_model_options_merged()?;
            if !cached.is_empty() {
                return Ok(ApiKeyModelListResult { items: cached });
            }
            let fallback = read_local_fallback_model_options()?;
            if !fallback.is_empty() {
                let _ = save_model_options_cache(&fallback);
                return Ok(ApiKeyModelListResult { items: fallback });
            }
            Err(err)
        }
    }
}

fn save_model_options_cache(items: &[ModelOption]) -> Result<(), String> {
    let storage =
        storage_helpers::open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    let items_json = serde_json::to_string(items).map_err(|e| e.to_string())?;
    storage
        .upsert_model_options_cache(MODEL_CACHE_SCOPE_DEFAULT, &items_json, now_ts())
        .map_err(|e| e.to_string())
}

fn read_cached_model_options() -> Result<Vec<ModelOption>, String> {
    let storage =
        storage_helpers::open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    let Some(cache) = storage
        .get_model_options_cache(MODEL_CACHE_SCOPE_DEFAULT)
        .map_err(|e| e.to_string())?
    else {
        return Ok(Vec::new());
    };
    let items = serde_json::from_str::<Vec<ModelOption>>(&cache.items_json).unwrap_or_default();
    Ok(items)
}

fn read_cached_model_options_merged() -> Result<Vec<ModelOption>, String> {
    let cached = read_cached_model_options()?;
    let configured = read_local_configured_model_options()?;
    if cached.is_empty() {
        return Ok(merge_model_options(default_model_options(), configured));
    }
    Ok(merge_model_options(cached, configured))
}

fn read_local_fallback_model_options() -> Result<Vec<ModelOption>, String> {
    Ok(merge_model_options(
        default_model_options(),
        read_local_configured_model_options()?,
    ))
}

fn read_local_configured_model_options() -> Result<Vec<ModelOption>, String> {
    let storage =
        storage_helpers::open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    let api_keys = storage.list_api_keys().map_err(|e| e.to_string())?;
    let items = api_keys
        .into_iter()
        .filter_map(|item| {
            let slug = item.model_slug?.trim().to_string();
            if slug.is_empty() {
                return None;
            }
            Some(ModelOption {
                display_name: slug.clone(),
                slug,
            })
        })
        .collect::<Vec<_>>();
    Ok(items)
}

fn default_model_options() -> Vec<ModelOption> {
    DEFAULT_MODEL_SLUGS
        .iter()
        .map(|slug| ModelOption {
            slug: (*slug).to_string(),
            display_name: (*slug).to_string(),
        })
        .collect()
}

fn merge_model_options(primary: Vec<ModelOption>, secondary: Vec<ModelOption>) -> Vec<ModelOption> {
    let mut merged = BTreeMap::<String, ModelOption>::new();
    for item in primary.into_iter().chain(secondary) {
        let slug = item.slug.trim();
        if slug.is_empty() {
            continue;
        }
        let display_name = item.display_name.trim();
        merged
            .entry(slug.to_string())
            .or_insert_with(|| ModelOption {
                slug: slug.to_string(),
                display_name: if display_name.is_empty() {
                    slug.to_string()
                } else {
                    display_name.to_string()
                },
            });
    }
    merged.into_values().collect()
}

#[cfg(test)]
#[path = "tests/apikey_models_tests.rs"]
mod tests;
