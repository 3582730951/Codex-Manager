use crate::apikey_profile::{PROTOCOL_ANTHROPIC_NATIVE, PROTOCOL_AZURE_OPENAI};
use crate::gateway::request_log::RequestLogUsage;
use std::time::Instant;
use tiny_http::Request;

use super::super::local_validation::LocalValidationResult;
use super::proxy_pipeline::candidate_executor::{
    execute_candidate_sequence, CandidateExecutionResult, CandidateExecutorParams,
};
use super::proxy_pipeline::execution_context::GatewayUpstreamExecutionContext;
use super::proxy_pipeline::request_setup::prepare_request_setup;
use super::proxy_pipeline::response_finalize::respond_terminal;
use super::support::precheck::{prepare_candidates_for_proxy, CandidatePrecheckResult};

pub(in super::super) fn proxy_validated_request(
    request: Request,
    validated: LocalValidationResult,
    debug: bool,
) -> Result<(), String> {
    let LocalValidationResult {
        trace_id,
        incoming_headers,
        storage,
        original_path,
        path,
        body,
        is_stream,
        has_prompt_cache_key,
        prompt_cache_key,
        request_shape,
        protocol_type,
        upstream_base_url,
        static_headers_json,
        response_adapter,
        tool_name_restore_map,
        request_method,
        key_id,
        model_for_log,
        reasoning_for_log,
        method,
    } = validated;
    let started_at = Instant::now();
    let client_is_stream = is_stream;
    let is_compact_path =
        path == "/v1/responses/compact" || path.starts_with("/v1/responses/compact?");
    // 中文注释：对齐 CPA：/v1/responses 上游固定走 SSE。
    // 下游是否流式仍由客户端 `stream` 参数决定（在 response bridge 层聚合/透传）。
    let upstream_is_stream =
        client_is_stream || (path.starts_with("/v1/responses") && !is_compact_path);
    let request_deadline = super::support::deadline::request_deadline(started_at, client_is_stream);

    super::super::trace_log::log_request_start(
        trace_id.as_str(),
        key_id.as_str(),
        request_method.as_str(),
        path.as_str(),
        model_for_log.as_deref(),
        reasoning_for_log.as_deref(),
        client_is_stream,
        protocol_type.as_str(),
    );
    super::super::trace_log::log_request_body_preview(trace_id.as_str(), body.as_ref());

    if protocol_type == PROTOCOL_AZURE_OPENAI {
        return super::protocol::azure_openai::proxy_azure_request(
            request,
            &storage,
            trace_id.as_str(),
            key_id.as_str(),
            original_path.as_str(),
            path.as_str(),
            request_method.as_str(),
            &method,
            &body,
            upstream_is_stream,
            response_adapter,
            &tool_name_restore_map,
            model_for_log.as_deref(),
            reasoning_for_log.as_deref(),
            upstream_base_url.as_deref(),
            static_headers_json.as_deref(),
            request_deadline,
            started_at,
        );
    }

    let (request, mut candidates) = match prepare_candidates_for_proxy(
        request,
        &storage,
        trace_id.as_str(),
        &key_id,
        &original_path,
        &path,
        response_adapter,
        &request_method,
        model_for_log.as_deref(),
        reasoning_for_log.as_deref(),
    ) {
        CandidatePrecheckResult::Ready {
            request,
            candidates,
        } => (request, candidates),
        CandidatePrecheckResult::Responded => return Ok(()),
    };
    let setup = prepare_request_setup(
        path.as_str(),
        protocol_type.as_str(),
        has_prompt_cache_key,
        prompt_cache_key.as_deref(),
        &incoming_headers,
        &body,
        &mut candidates,
        key_id.as_str(),
        model_for_log.as_deref(),
        trace_id.as_str(),
    );
    let base = setup.upstream_base.as_str();

    let context = GatewayUpstreamExecutionContext::new(
        &trace_id,
        &storage,
        &key_id,
        &original_path,
        &path,
        &request_method,
        response_adapter,
        protocol_type.as_str(),
        model_for_log.as_deref(),
        reasoning_for_log.as_deref(),
        setup.candidate_count,
        setup.account_max_inflight,
    );
    let allow_openai_fallback = false;
    let disable_challenge_stateless_retry = !(protocol_type == PROTOCOL_ANTHROPIC_NATIVE
        && body.len() <= 2 * 1024)
        && !path.starts_with("/v1/responses");
    let request = match execute_candidate_sequence(
        request,
        candidates,
        CandidateExecutorParams {
            storage: &storage,
            method: &method,
            incoming_headers: &incoming_headers,
            body: &body,
            path: path.as_str(),
            request_shape: request_shape.as_deref(),
            trace_id: trace_id.as_str(),
            model_for_log: model_for_log.as_deref(),
            response_adapter,
            tool_name_restore_map: &tool_name_restore_map,
            context: &context,
            setup: &setup,
            request_deadline,
            started_at,
            client_is_stream,
            upstream_is_stream,
            debug,
            allow_openai_fallback,
            disable_challenge_stateless_retry,
        },
    )? {
        CandidateExecutionResult::Handled => return Ok(()),
        CandidateExecutionResult::Exhausted(request) => request,
    };

    context.log_final_result(
        None,
        Some(base),
        503,
        RequestLogUsage::default(),
        Some("candidate exhausted"),
        started_at.elapsed().as_millis(),
        None,
    );
    respond_terminal(
        request,
        503,
        "no available account".to_string(),
        Some(trace_id.as_str()),
    )
}
