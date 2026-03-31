use crate::apikey_profile::{PROTOCOL_ANTHROPIC_NATIVE, ROTATION_AGGREGATE_API};
use bytes::Bytes;
use codexmanager_core::storage::{ApiKey, ApiKeyOwnerContext};
use reqwest::Method;
use tiny_http::Request;

use super::{LocalValidationError, LocalValidationResult};

fn resolve_effective_request_overrides(
    api_key: &ApiKey,
) -> (Option<String>, Option<String>, Option<String>) {
    let normalized_model = api_key
        .model_slug
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let normalized_reasoning = api_key
        .reasoning_effort
        .as_deref()
        .and_then(crate::reasoning_effort::normalize_reasoning_effort)
        .map(str::to_string);
    let normalized_service_tier = api_key
        .service_tier
        .as_deref()
        .and_then(crate::apikey::service_tier::normalize_service_tier)
        .map(str::to_string);

    (
        normalized_model,
        normalized_reasoning,
        normalized_service_tier,
    )
}

fn inherit_parent_runtime_config(child_key: &ApiKey, parent_key: &ApiKey) -> ApiKey {
    ApiKey {
        id: child_key.id.clone(),
        name: child_key.name.clone(),
        model_slug: parent_key.model_slug.clone(),
        reasoning_effort: parent_key.reasoning_effort.clone(),
        service_tier: parent_key.service_tier.clone(),
        rotation_strategy: parent_key.rotation_strategy.clone(),
        aggregate_api_id: parent_key.aggregate_api_id.clone(),
        aggregate_api_url: parent_key.aggregate_api_url.clone(),
        client_type: parent_key.client_type.clone(),
        protocol_type: parent_key.protocol_type.clone(),
        auth_scheme: parent_key.auth_scheme.clone(),
        upstream_base_url: parent_key.upstream_base_url.clone(),
        static_headers_json: parent_key.static_headers_json.clone(),
        key_hash: child_key.key_hash.clone(),
        status: child_key.status.clone(),
        created_at: child_key.created_at,
        last_used_at: child_key.last_used_at,
    }
}

fn resolve_runtime_api_key(
    storage: &crate::storage_helpers::StorageHandle,
    presented_api_key: &ApiKey,
    owner_context: Option<&ApiKeyOwnerContext>,
) -> Result<ApiKey, LocalValidationError> {
    let Some(owner_context) = owner_context else {
        return Ok(presented_api_key.clone());
    };
    let parent_key = storage
        .find_api_key_by_id(owner_context.owner_key_id.as_str())
        .map_err(|err| LocalValidationError::new(500, format!("storage read failed: {err}")))?
        .ok_or_else(|| LocalValidationError::new(403, "parent api key is missing"))?;
    if parent_key.status != "active" {
        return Err(LocalValidationError::new(403, "parent api key disabled"));
    }
    Ok(inherit_parent_runtime_config(
        presented_api_key,
        &parent_key,
    ))
}

fn allow_openai_responses_path_rewrite(protocol_type: &str, normalized_path: &str) -> bool {
    protocol_type == crate::apikey_profile::PROTOCOL_OPENAI_COMPAT
        && (normalized_path.starts_with("/v1/chat/completions")
            || normalized_path.starts_with("/v1/completions"))
}

fn apply_passthrough_request_overrides(
    path: &str,
    body: Vec<u8>,
    api_key: &ApiKey,
) -> (
    Vec<u8>,
    Option<String>,
    Option<String>,
    bool,
    Option<String>,
) {
    let (effective_model, effective_reasoning, effective_service_tier) =
        resolve_effective_request_overrides(api_key);
    let rewritten_body =
        super::super::apply_request_overrides_with_service_tier_and_prompt_cache_key(
            path,
            body,
            effective_model.as_deref(),
            effective_reasoning.as_deref(),
            effective_service_tier.as_deref(),
            api_key.upstream_base_url.as_deref(),
            None,
        );
    let request_meta = super::super::parse_request_metadata(&rewritten_body);
    (
        rewritten_body,
        request_meta.model.or(api_key.model_slug.clone()),
        request_meta
            .reasoning_effort
            .or(api_key.reasoning_effort.clone()),
        request_meta.has_prompt_cache_key,
        request_meta.request_shape,
    )
}

pub(super) fn build_local_validation_result(
    request: &Request,
    trace_id: String,
    incoming_headers: super::super::IncomingHeaderSnapshot,
    storage: crate::storage_helpers::StorageHandle,
    mut body: Vec<u8>,
    presented_api_key: ApiKey,
) -> Result<LocalValidationResult, LocalValidationError> {
    // 按当前策略取消每次请求都更新 api_keys.last_used_at，减少并发写入冲突。
    let normalized_path = super::super::normalize_models_path(request.url());
    let request_method = request.method().as_str().to_string();
    let method = Method::from_bytes(request_method.as_bytes())
        .map_err(|_| LocalValidationError::new(405, "unsupported method"))?;
    let owner_context = storage
        .lookup_api_key_owner_context(presented_api_key.id.as_str())
        .map_err(|err| LocalValidationError::new(500, format!("storage read failed: {err}")))?;
    let effective_api_key =
        resolve_runtime_api_key(&storage, &presented_api_key, owner_context.as_ref())?;
    let cli_affinity_override = owner_context
        .as_ref()
        .map(|context| context.cli_instance_uuid.clone());
    let initial_request_meta = super::super::parse_request_metadata(&body);
    let incoming_headers =
        incoming_headers.with_cli_affinity_id_override(cli_affinity_override.as_deref());
    let initial_local_conversation_id = incoming_headers.conversation_id().map(str::to_string);

    if effective_api_key.rotation_strategy == ROTATION_AGGREGATE_API {
        let (rewritten_body, model_for_log, reasoning_for_log, has_prompt_cache_key, request_shape) =
            apply_passthrough_request_overrides(&normalized_path, body, &effective_api_key);
        let incoming_headers = incoming_headers
            .with_conversation_id_override(initial_local_conversation_id.as_deref());
        return Ok(LocalValidationResult {
            trace_id,
            incoming_headers,
            storage,
            original_path: normalized_path.clone(),
            path: normalized_path,
            body: Bytes::from(rewritten_body),
            is_stream: initial_request_meta.is_stream,
            has_prompt_cache_key,
            request_shape,
            protocol_type: effective_api_key.protocol_type,
            rotation_strategy: effective_api_key.rotation_strategy,
            aggregate_api_id: effective_api_key.aggregate_api_id,
            upstream_base_url: effective_api_key.upstream_base_url,
            static_headers_json: effective_api_key.static_headers_json,
            response_adapter: super::super::ResponseAdapter::Passthrough,
            tool_name_restore_map: super::super::ToolNameRestoreMap::default(),
            request_method,
            key_id: presented_api_key.id,
            owner_key_id: owner_context
                .as_ref()
                .map(|context| context.owner_key_id.clone()),
            cli_instance_uuid: owner_context
                .as_ref()
                .map(|context| context.cli_instance_uuid.clone()),
            platform_key_hash: presented_api_key.key_hash,
            local_conversation_id: initial_local_conversation_id,
            model_for_log,
            reasoning_for_log,
            method,
        });
    }

    let original_body = body.clone();
    let adapted = super::super::adapt_request_for_protocol(
        effective_api_key.protocol_type.as_str(),
        &normalized_path,
        body,
    )
    .map_err(|err| LocalValidationError::new(400, err))?;
    let mut path = adapted.path;
    let mut response_adapter = adapted.response_adapter;
    let mut tool_name_restore_map = adapted.tool_name_restore_map;
    body = adapted.body;
    if effective_api_key.protocol_type != PROTOCOL_ANTHROPIC_NATIVE
        && !normalized_path.starts_with("/v1/responses")
        && path.starts_with("/v1/responses")
        && !allow_openai_responses_path_rewrite(&effective_api_key.protocol_type, &normalized_path)
    {
        // 中文注释：防回归保护：仅 anthropic_native 的 /v1/messages 允许改写到 /v1/responses；
        // 其余协议和路径一律保持原路径透传，避免客户端按 chat/completions 语义却拿到 responses 流格式。
        log::warn!(
            "event=gateway_protocol_adapt_guard protocol_type={} from_path={} to_path={} action=force_passthrough",
            effective_api_key.protocol_type,
            normalized_path,
            path
        );
        path = normalized_path.clone();
        body = original_body;
        response_adapter = super::super::ResponseAdapter::Passthrough;
        tool_name_restore_map.clear();
    }
    // 中文注释：下游调用方的 stream / shape 语义必须以原始请求体为准；
    // anthropic -> responses 改写会强制上游 stream=true，不能反向污染下游响应模式与日志。
    let client_request_meta = initial_request_meta.clone();
    let (effective_model, effective_reasoning, effective_service_tier) =
        resolve_effective_request_overrides(&effective_api_key);
    let local_conversation_id = incoming_headers.conversation_id().map(str::to_string);
    let incoming_headers =
        incoming_headers.with_conversation_id_override(local_conversation_id.as_deref());
    body = super::super::apply_request_overrides_with_service_tier_and_prompt_cache_key(
        &path,
        body,
        effective_model.as_deref(),
        effective_reasoning.as_deref(),
        effective_service_tier.as_deref(),
        effective_api_key.upstream_base_url.as_deref(),
        None,
    );

    let request_meta = super::super::parse_request_metadata(&body);
    let model_for_log = request_meta.model.or(effective_api_key.model_slug.clone());
    let reasoning_for_log = request_meta
        .reasoning_effort
        .or(effective_api_key.reasoning_effort.clone());
    let is_stream = client_request_meta.is_stream;
    let has_prompt_cache_key = client_request_meta.has_prompt_cache_key;
    let request_shape = client_request_meta.request_shape;

    Ok(LocalValidationResult {
        trace_id,
        incoming_headers,
        storage,
        original_path: normalized_path,
        path,
        body: Bytes::from(body),
        is_stream,
        has_prompt_cache_key,
        request_shape,
        protocol_type: effective_api_key.protocol_type,
        upstream_base_url: effective_api_key.upstream_base_url,
        static_headers_json: effective_api_key.static_headers_json,
        response_adapter,
        tool_name_restore_map,
        request_method,
        key_id: presented_api_key.id,
        owner_key_id: owner_context
            .as_ref()
            .map(|context| context.owner_key_id.clone()),
        cli_instance_uuid: owner_context
            .as_ref()
            .map(|context| context.cli_instance_uuid.clone()),
        platform_key_hash: presented_api_key.key_hash,
        local_conversation_id,
        rotation_strategy: effective_api_key.rotation_strategy,
        aggregate_api_id: effective_api_key.aggregate_api_id,
        model_for_log,
        reasoning_for_log,
        method,
    })
}

#[cfg(test)]
#[path = "tests/request_tests.rs"]
mod tests;
