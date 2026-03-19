use serde_json::json;
use serde_json::Value;

pub(crate) const ERROR_CODE_HEADER_NAME: &str = "X-CodexManager-Error-Code";
pub(crate) const TRACE_ID_HEADER_NAME: &str = "X-CodexManager-Trace-Id";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorCode {
    UnknownMethod,
    UnknownError,
    InvalidSettingsPayload,
    InvalidRequestPayload,
    ProtocolMappingError,
    RequestBodyTooLarge,
    BackendProxyError,
    BuildResponseFailed,
    QueueShed,
    GateRejected,
    UpstreamTimeout,
    UpstreamConnectFailure,
    UpstreamPreheaderTimeout,
    UpstreamChallengeBlocked,
    UpstreamRateLimited,
    UpstreamNotFound,
    UpstreamNonSuccess,
    NoAvailableAccount,
    CandidateResolveFailed,
    ClientCancelled,
    GatewayBridgeError,
    ResponseWriteFailed,
    StreamInterrupted,
}

impl ErrorCode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownMethod => "unknown_method",
            Self::UnknownError => "unknown_error",
            Self::InvalidSettingsPayload => "invalid_settings_payload",
            Self::InvalidRequestPayload => "invalid_request_payload",
            Self::ProtocolMappingError => "protocol_mapping_error",
            Self::RequestBodyTooLarge => "request_body_too_large",
            Self::BackendProxyError => "backend_proxy_error",
            Self::BuildResponseFailed => "build_response_failed",
            Self::QueueShed => "queue_shed",
            Self::GateRejected => "gate_rejected",
            Self::UpstreamTimeout => "upstream_timeout",
            Self::UpstreamConnectFailure => "upstream_connect_failure",
            Self::UpstreamPreheaderTimeout => "upstream_preheader_timeout",
            Self::UpstreamChallengeBlocked => "upstream_challenge_blocked",
            Self::UpstreamRateLimited => "upstream_rate_limited",
            Self::UpstreamNotFound => "upstream_not_found",
            Self::UpstreamNonSuccess => "upstream_non_success",
            Self::NoAvailableAccount => "no_available_account",
            Self::CandidateResolveFailed => "candidate_resolve_failed",
            Self::ClientCancelled => "client_cancelled",
            Self::GatewayBridgeError => "gateway_bridge_error",
            Self::ResponseWriteFailed => "response_write_failed",
            Self::StreamInterrupted => "stream_interrupted",
        }
    }
}

pub(crate) fn classify_message(message: &str) -> ErrorCode {
    let normalized = message.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return ErrorCode::UnknownError;
    }

    if normalized == "unknown_method" {
        return ErrorCode::UnknownMethod;
    }
    if normalized.starts_with("invalid app settings payload:") {
        return ErrorCode::InvalidSettingsPayload;
    }
    if normalized.starts_with("request body too large") {
        return ErrorCode::RequestBodyTooLarge;
    }
    if normalized.starts_with("backend proxy error:") {
        return ErrorCode::BackendProxyError;
    }
    if normalized.starts_with("build response failed:") {
        return ErrorCode::BuildResponseFailed;
    }
    if normalized.starts_with("response conversion failed:") {
        return ErrorCode::ProtocolMappingError;
    }
    if normalized.contains("queue is saturated") || normalized.contains("workers are unavailable") {
        return ErrorCode::QueueShed;
    }
    if normalized == "request gate rejected" {
        return ErrorCode::GateRejected;
    }
    if normalized == "upstream total timeout exceeded" {
        return ErrorCode::UpstreamTimeout;
    }
    if normalized == "upstream_stream_idle_timeout"
        || normalized.starts_with("upstream_stream_idle_timeout:")
    {
        return ErrorCode::UpstreamTimeout;
    }
    if normalized == "upstream_connect_failure" {
        return ErrorCode::UpstreamConnectFailure;
    }
    if normalized == "upstream_preheader_timeout" {
        return ErrorCode::UpstreamPreheaderTimeout;
    }
    if normalized == "上游请求超时"
        || normalized.starts_with("上游流空闲超时")
        || normalized.contains("连接超时")
    {
        return ErrorCode::UpstreamTimeout;
    }
    if normalized.starts_with("upstream blocked by cloudflare/waf")
        || normalized == "upstream challenge blocked"
    {
        return ErrorCode::UpstreamChallengeBlocked;
    }
    if normalized.contains("cloudflare/waf")
        || normalized.contains("安全验证拦截")
        || normalized.contains("验证/拦截页面")
    {
        return ErrorCode::UpstreamChallengeBlocked;
    }
    if normalized == "upstream rate-limited" {
        return ErrorCode::UpstreamRateLimited;
    }
    if normalized == "upstream not-found failover" {
        return ErrorCode::UpstreamNotFound;
    }
    if normalized == "upstream_stream_terminal_error"
        || normalized.starts_with("upstream_stream_terminal_error:")
    {
        return ErrorCode::UpstreamNonSuccess;
    }
    if normalized == "upstream_disconnect_before_terminal"
        || normalized.starts_with("upstream_disconnect_before_terminal:")
        || normalized == "stream_incomplete_before_output"
        || normalized.starts_with("stream_incomplete_before_output:")
        || normalized == "stream_incomplete_after_output"
        || normalized.starts_with("stream_incomplete_after_output:")
        || normalized == "stream_incomplete_unknown"
        || normalized.starts_with("stream_incomplete_unknown:")
    {
        return ErrorCode::StreamInterrupted;
    }
    if normalized == "upstream non-success" {
        return ErrorCode::UpstreamNonSuccess;
    }
    if normalized == "no available account" {
        return ErrorCode::NoAvailableAccount;
    }
    if normalized == "candidate exhausted" {
        return ErrorCode::NoAvailableAccount;
    }
    if normalized.starts_with("candidate resolve failed:") {
        return ErrorCode::CandidateResolveFailed;
    }
    if normalized == "client_cancelled" {
        return ErrorCode::ClientCancelled;
    }
    if normalized == "gateway_bridge_error" || normalized.starts_with("gateway_bridge_error:") {
        return ErrorCode::GatewayBridgeError;
    }
    if normalized.starts_with("response write failed:") {
        return ErrorCode::ResponseWriteFailed;
    }
    if normalized == "stream disconnected before completion" {
        return ErrorCode::StreamInterrupted;
    }
    if normalized.starts_with("上游流中途中断")
        || normalized.starts_with("上游流读取失败（连接中断）")
        || normalized.contains("上游连接中断")
    {
        return ErrorCode::StreamInterrupted;
    }
    if normalized.starts_with("上游返回的不是正常接口数据")
        || normalized.starts_with("上游返回了网页内容而不是接口数据")
    {
        return ErrorCode::UpstreamNonSuccess;
    }
    if normalized.starts_with("模型不支持") {
        return ErrorCode::UpstreamNonSuccess;
    }
    if normalized.starts_with("invalid upstream ")
        || (normalized.contains("serialize") && normalized.contains("json"))
        || normalized.contains("sse bytes")
    {
        return ErrorCode::ProtocolMappingError;
    }
    if normalized == "invalid claude request json"
        || normalized == "claude request body must be an object"
    {
        return ErrorCode::InvalidRequestPayload;
    }

    ErrorCode::UnknownError
}

pub(crate) fn code_or_dash(message: Option<&str>) -> &'static str {
    message
        .map(classify_message)
        .map(ErrorCode::as_str)
        .unwrap_or("-")
}

pub(crate) fn code_for_message(message: &str) -> &'static str {
    classify_message(message).as_str()
}

pub(crate) fn rpc_error_payload(message: String) -> Value {
    let code = classify_message(message.as_str()).as_str();
    json!({
        "error": message,
        "errorCode": code,
        "errorDetail": {
            "code": code,
            "message": message,
        }
    })
}

pub(crate) fn rpc_action_error_payload(message: String) -> Value {
    let code = classify_message(message.as_str()).as_str();
    json!({
        "ok": false,
        "error": message,
        "errorCode": code,
        "errorDetail": {
            "code": code,
            "message": message,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{classify_message, ErrorCode};

    #[test]
    fn classify_known_messages() {
        assert_eq!(
            classify_message("invalid app settings payload: missing field"),
            ErrorCode::InvalidSettingsPayload
        );
        assert_eq!(
            classify_message("upstream total timeout exceeded"),
            ErrorCode::UpstreamTimeout
        );
        assert_eq!(
            classify_message("invalid upstream json payload"),
            ErrorCode::ProtocolMappingError
        );
        assert_eq!(
            classify_message("backend proxy error: connection refused"),
            ErrorCode::BackendProxyError
        );
        assert_eq!(
            classify_message("claude request body must be an object"),
            ErrorCode::InvalidRequestPayload
        );
        assert_eq!(classify_message("上游请求超时"), ErrorCode::UpstreamTimeout);
        assert_eq!(
            classify_message("上游被安全验证拦截（Cloudflare/WAF）"),
            ErrorCode::UpstreamChallengeBlocked
        );
        assert_eq!(
            classify_message("上游流中途中断（未正常结束）"),
            ErrorCode::StreamInterrupted
        );
        assert_eq!(
            classify_message("模型不支持（gpt-5.4）"),
            ErrorCode::UpstreamNonSuccess
        );
        assert_eq!(
            classify_message("queue is saturated, try again later"),
            ErrorCode::QueueShed
        );
        assert_eq!(
            classify_message("workers are unavailable"),
            ErrorCode::QueueShed
        );
        assert_eq!(
            classify_message("request gate rejected"),
            ErrorCode::GateRejected
        );
        assert_eq!(
            classify_message("upstream_connect_failure"),
            ErrorCode::UpstreamConnectFailure
        );
        assert_eq!(
            classify_message("upstream_preheader_timeout"),
            ErrorCode::UpstreamPreheaderTimeout
        );
        assert_eq!(
            classify_message("candidate exhausted"),
            ErrorCode::NoAvailableAccount
        );
        assert_eq!(
            classify_message("upstream_stream_terminal_error: code=server_error"),
            ErrorCode::UpstreamNonSuccess
        );
        assert_eq!(
            classify_message("stream_incomplete_unknown"),
            ErrorCode::StreamInterrupted
        );
        assert_eq!(
            classify_message("stream_incomplete_before_output: 上游流中途中断（未正常结束）"),
            ErrorCode::StreamInterrupted
        );
        assert_eq!(
            classify_message("upstream_stream_idle_timeout: 上游流空闲超时"),
            ErrorCode::UpstreamTimeout
        );
        assert_eq!(
            classify_message("client_cancelled"),
            ErrorCode::ClientCancelled
        );
        assert_eq!(
            classify_message("gateway_bridge_error"),
            ErrorCode::GatewayBridgeError
        );
        assert_eq!(
            classify_message("gateway_bridge_error: content_type_mismatch"),
            ErrorCode::GatewayBridgeError
        );
    }
}
