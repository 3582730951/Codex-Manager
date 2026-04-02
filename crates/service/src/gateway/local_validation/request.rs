use crate::apikey_profile::{PROTOCOL_ANTHROPIC_NATIVE, ROTATION_AGGREGATE_API};
use codexmanager_core::storage::{ApiKey, ApiKeyOwnerContext};
use reqwest::Method;
use tiny_http::Request;

use super::{LocalValidationError, LocalValidationResult};

#[derive(Debug)]
struct PreparedGatewayRequest {
    path: String,
    body: crate::gateway::RequestPayload,
    response_adapter: super::super::ResponseAdapter,
    tool_name_restore_map: super::super::ToolNameRestoreMap,
    request_meta: super::super::request_helpers::ParsedRequestMetadata,
}

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
    body: &crate::gateway::RequestPayload,
    api_key: &ApiKey,
) -> Result<PreparedGatewayRequest, LocalValidationError> {
    let (effective_model, effective_reasoning, effective_service_tier) =
        resolve_effective_request_overrides(api_key);
    let rewritten_body =
        super::super::apply_request_overrides_payload_with_service_tier_and_prompt_cache_key(
            path,
            body,
            effective_model.as_deref(),
            effective_reasoning.as_deref(),
            effective_service_tier.as_deref(),
            api_key.upstream_base_url.as_deref(),
            None,
        )
        .map_err(|err| LocalValidationError::new(400, err))?;
    let request_meta = super::super::parse_request_metadata_payload(&rewritten_body);
    Ok(PreparedGatewayRequest {
        path: path.to_string(),
        body: rewritten_body,
        response_adapter: super::super::ResponseAdapter::Passthrough,
        tool_name_restore_map: super::super::ToolNameRestoreMap::default(),
        request_meta,
    })
}

fn prepare_runtime_request(
    normalized_path: &str,
    original_body: &crate::gateway::RequestPayload,
    effective_api_key: &ApiKey,
) -> Result<PreparedGatewayRequest, LocalValidationError> {
    let mut path = normalized_path.to_string();
    let mut response_adapter = super::super::ResponseAdapter::Passthrough;
    let mut tool_name_restore_map = super::super::ToolNameRestoreMap::default();
    let mut working_body = original_body.clone();
    let requires_protocol_adaptation = effective_api_key.protocol_type
        != crate::apikey_profile::PROTOCOL_OPENAI_COMPAT
        || normalized_path.starts_with("/v1/chat/completions")
        || normalized_path.starts_with("/v1/completions")
        || normalized_path.starts_with("/v1/messages");
    if requires_protocol_adaptation {
        let original_bytes = original_body.read_all_bytes().map_err(|err| {
            LocalValidationError::new(400, format!("read request body failed: {err}"))
        })?;
        let adapted = super::super::adapt_request_for_protocol(
            effective_api_key.protocol_type.as_str(),
            normalized_path,
            original_bytes.to_vec(),
        )
        .map_err(|err| LocalValidationError::new(400, err))?;
        path = adapted.path;
        response_adapter = adapted.response_adapter;
        tool_name_restore_map = adapted.tool_name_restore_map;
        working_body = crate::gateway::RequestPayload::from_vec(adapted.body)
            .map_err(|err| LocalValidationError::new(400, err))?;
    }
    if effective_api_key.protocol_type != PROTOCOL_ANTHROPIC_NATIVE
        && !normalized_path.starts_with("/v1/responses")
        && path.starts_with("/v1/responses")
        && !allow_openai_responses_path_rewrite(&effective_api_key.protocol_type, normalized_path)
    {
        log::warn!(
            "event=gateway_protocol_adapt_guard protocol_type={} from_path={} to_path={} action=force_passthrough",
            effective_api_key.protocol_type,
            normalized_path,
            path
        );
        path = normalized_path.to_string();
        working_body = original_body.clone();
        response_adapter = super::super::ResponseAdapter::Passthrough;
        tool_name_restore_map.clear();
    }

    let (effective_model, effective_reasoning, effective_service_tier) =
        resolve_effective_request_overrides(effective_api_key);
    working_body =
        super::super::apply_request_overrides_payload_with_service_tier_and_prompt_cache_key(
            &path,
            &working_body,
            effective_model.as_deref(),
            effective_reasoning.as_deref(),
            effective_service_tier.as_deref(),
            effective_api_key.upstream_base_url.as_deref(),
            None,
        )
        .map_err(|err| LocalValidationError::new(400, err))?;

    let request_meta = super::super::parse_request_metadata_payload(&working_body);

    Ok(PreparedGatewayRequest {
        path,
        body: working_body,
        response_adapter,
        tool_name_restore_map,
        request_meta,
    })
}

pub(super) fn build_local_validation_result(
    request: &Request,
    trace_id: String,
    incoming_headers: super::super::IncomingHeaderSnapshot,
    storage: crate::storage_helpers::StorageHandle,
    body: crate::gateway::RequestPayload,
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
    let initial_request_meta = super::super::parse_request_metadata_payload(&body);
    let incoming_headers =
        incoming_headers.with_cli_affinity_id_override(cli_affinity_override.as_deref());
    let initial_local_conversation_id = incoming_headers.conversation_id().map(str::to_string);

    if effective_api_key.rotation_strategy == ROTATION_AGGREGATE_API {
        let prepared =
            apply_passthrough_request_overrides(&normalized_path, &body, &effective_api_key)?;
        let incoming_headers = incoming_headers
            .with_conversation_id_override(initial_local_conversation_id.as_deref());
        return Ok(LocalValidationResult {
            trace_id,
            incoming_headers,
            storage,
            original_path: normalized_path.clone(),
            path: prepared.path,
            body: prepared.body,
            is_stream: initial_request_meta.is_stream,
            has_prompt_cache_key: prepared.request_meta.has_prompt_cache_key,
            request_shape: prepared.request_meta.request_shape.clone(),
            protocol_type: effective_api_key.protocol_type,
            rotation_strategy: effective_api_key.rotation_strategy,
            aggregate_api_id: effective_api_key.aggregate_api_id,
            upstream_base_url: effective_api_key.upstream_base_url,
            static_headers_json: effective_api_key.static_headers_json,
            response_adapter: prepared.response_adapter,
            tool_name_restore_map: prepared.tool_name_restore_map,
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
            model_for_log: prepared
                .request_meta
                .model
                .or(effective_api_key.model_slug.clone()),
            reasoning_for_log: prepared
                .request_meta
                .reasoning_effort
                .or(effective_api_key.reasoning_effort.clone()),
            method,
        });
    }
    // 中文注释：下游调用方的 stream / shape 语义必须以原始请求体为准；
    // anthropic -> responses 改写会强制上游 stream=true，不能反向污染下游响应模式与日志。
    let client_request_meta = initial_request_meta.clone();
    let prepared = prepare_runtime_request(&normalized_path, &body, &effective_api_key)?;
    let local_conversation_id = incoming_headers.conversation_id().map(str::to_string);
    let incoming_headers =
        incoming_headers.with_conversation_id_override(local_conversation_id.as_deref());
    let model_for_log = prepared
        .request_meta
        .model
        .or(effective_api_key.model_slug.clone());
    let reasoning_for_log = prepared
        .request_meta
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
        path: prepared.path,
        body: prepared.body,
        is_stream,
        has_prompt_cache_key,
        request_shape,
        protocol_type: effective_api_key.protocol_type,
        upstream_base_url: effective_api_key.upstream_base_url,
        static_headers_json: effective_api_key.static_headers_json,
        response_adapter: prepared.response_adapter,
        tool_name_restore_map: prepared.tool_name_restore_map,
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
