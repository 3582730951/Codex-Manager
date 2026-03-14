use reqwest::header::HeaderValue;
use std::sync::{OnceLock, RwLock};

const ENV_UPSTREAM_BASE_URL: &str = "CODEXMANAGER_UPSTREAM_BASE_URL";
const DEFAULT_UPSTREAM_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

static CONFIG_LOADED: OnceLock<()> = OnceLock::new();
static UPSTREAM_BASE_URL: OnceLock<RwLock<String>> = OnceLock::new();

pub(in super::super) fn normalize_upstream_base_url(base: &str) -> String {
    let mut normalized = base.trim().trim_end_matches('/').to_string();
    let lower = normalized.to_ascii_lowercase();
    if (lower.starts_with("https://chatgpt.com") || lower.starts_with("https://chat.openai.com"))
        && !lower.contains("/backend-api")
    {
        // 中文注释：对齐官方客户端的主机归一化，避免仅填域名时落到错误路径。
        normalized = format!("{normalized}/backend-api/codex");
    }
    normalized
}

pub(in super::super) fn resolve_upstream_base_url() -> String {
    ensure_config_loaded();
    crate::lock_utils::read_recover(upstream_base_url_cell(), "upstream_base_url").clone()
}

pub(in super::super) fn resolve_upstream_fallback_base_url(_primary_base: &str) -> Option<String> {
    None
}

pub(in super::super) fn is_openai_api_base(base: &str) -> bool {
    let normalized = base.trim().to_ascii_lowercase();
    normalized.contains("api.openai.com/v1")
}

pub(in super::super) fn is_chatgpt_backend_base(base: &str) -> bool {
    let normalized = base.trim().to_ascii_lowercase();
    normalized.contains("chatgpt.com/backend-api")
        || normalized.contains("chat.openai.com/backend-api")
}

pub(in super::super) fn should_try_openai_fallback(
    base: &str,
    request_path: &str,
    content_type: Option<&HeaderValue>,
) -> bool {
    if !is_chatgpt_backend_base(base) {
        return false;
    }
    let is_responses_path = request_path.starts_with("/v1/responses");
    let is_chat_completions_path = request_path.starts_with("/v1/chat/completions");
    if !is_responses_path && !is_chat_completions_path {
        // 仅对 /responses 和 /chat/completions 评估 OpenAI fallback；
        // 其余路径保持原有行为，避免扩大 fallback 面。
        return false;
    }
    let Some(content_type) = content_type else {
        return false;
    };
    let Ok(value) = content_type.to_str() else {
        return false;
    };
    // 中文注释：/chat/completions 仅在明确命中 HTML challenge 时才允许 fallback，
    // 避免仅凭状态码把普通业务错误错误地切到 OpenAI API。
    super::super::is_html_content_type(value)
}

pub(in super::super) fn should_try_openai_fallback_by_status(
    base: &str,
    request_path: &str,
    status_code: u16,
) -> bool {
    if !is_chatgpt_backend_base(base) {
        return false;
    }
    let is_responses_path = request_path.starts_with("/v1/responses");
    if !is_responses_path {
        return false;
    }
    if status_code == 429 {
        return true;
    }
    if status_code == 401 || status_code == 403 {
        // /v1/responses 在部分账号上会先返回 401/403（content-type 未必是 text/html），
        // 若只依赖 content-type 触发 fallback，会直接落到 challenge blocked。
        return true;
    }
    false
}

pub(in super::super) fn reload_from_env() {
    let base = env_non_empty(ENV_UPSTREAM_BASE_URL)
        .map(|value| normalize_upstream_base_url(&value))
        .unwrap_or_else(|| DEFAULT_UPSTREAM_BASE_URL.to_string());
    let mut cached_base =
        crate::lock_utils::write_recover(upstream_base_url_cell(), "upstream_base_url");
    *cached_base = base;
}

fn ensure_config_loaded() {
    let _ = CONFIG_LOADED.get_or_init(|| reload_from_env());
}

fn upstream_base_url_cell() -> &'static RwLock<String> {
    UPSTREAM_BASE_URL.get_or_init(|| RwLock::new(DEFAULT_UPSTREAM_BASE_URL.to_string()))
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
#[path = "tests/config_tests.rs"]
mod tests;
