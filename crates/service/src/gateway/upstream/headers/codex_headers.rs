use super::sticky_ids::random_session_id;

pub(crate) const CODEX_CLIENT_VERSION: &str = "0.101.0";

pub(crate) struct CodexUpstreamHeaderInput<'a> {
    pub(crate) auth_token: &'a str,
    pub(crate) account_id: Option<&'a str>,
    pub(crate) include_account_id: bool,
    pub(crate) upstream_cookie: Option<&'a str>,
    pub(crate) incoming_session_id: Option<&'a str>,
    pub(crate) incoming_client_request_id: Option<&'a str>,
    pub(crate) incoming_subagent: Option<&'a str>,
    pub(crate) incoming_beta_features: Option<&'a str>,
    pub(crate) incoming_turn_metadata: Option<&'a str>,
    pub(crate) fallback_session_id: Option<&'a str>,
    pub(crate) incoming_turn_state: Option<&'a str>,
    pub(crate) include_turn_state: bool,
    pub(crate) strip_session_affinity: bool,
    pub(crate) is_stream: bool,
    pub(crate) has_body: bool,
}

pub(crate) struct CodexCompactUpstreamHeaderInput<'a> {
    pub(crate) auth_token: &'a str,
    pub(crate) account_id: Option<&'a str>,
    pub(crate) include_account_id: bool,
    pub(crate) upstream_cookie: Option<&'a str>,
    pub(crate) incoming_session_id: Option<&'a str>,
    pub(crate) incoming_subagent: Option<&'a str>,
    pub(crate) fallback_session_id: Option<&'a str>,
    pub(crate) strip_session_affinity: bool,
    pub(crate) has_body: bool,
}

pub(crate) fn build_codex_upstream_headers(
    input: CodexUpstreamHeaderInput<'_>,
) -> Vec<(String, String)> {
    let mut headers = Vec::with_capacity(10);
    let resolved_session_id = resolve_session_id(
        input.incoming_session_id,
        input.fallback_session_id,
        input.strip_session_affinity,
    );
    headers.push((
        "Authorization".to_string(),
        format!("Bearer {}", input.auth_token),
    ));
    if input.has_body {
        headers.push(("Content-Type".to_string(), "application/json".to_string()));
    }
    headers.push((
        "Accept".to_string(),
        if input.is_stream {
            "text/event-stream"
        } else {
            "application/json"
        }
        .to_string(),
    ));
    headers.push((
        "User-Agent".to_string(),
        crate::gateway::current_codex_user_agent(),
    ));
    headers.push((
        "originator".to_string(),
        crate::gateway::current_originator(),
    ));
    if let Some(residency_requirement) = crate::gateway::current_residency_requirement() {
        headers.push((
            crate::gateway::runtime_config::RESIDENCY_HEADER_NAME.to_string(),
            residency_requirement,
        ));
    }
    if let Some(client_request_id) = resolve_client_request_id(
        input.incoming_client_request_id,
    ) {
        headers.push(("x-client-request-id".to_string(), client_request_id));
    }
    if let Some(subagent) = input
        .incoming_subagent
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        headers.push(("x-openai-subagent".to_string(), subagent.to_string()));
    }
    if let Some(beta_features) = input
        .incoming_beta_features
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        headers.push((
            "x-codex-beta-features".to_string(),
            beta_features.to_string(),
        ));
    }
    if let Some(turn_metadata) = input
        .incoming_turn_metadata
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        headers.push((
            "x-codex-turn-metadata".to_string(),
            turn_metadata.to_string(),
        ));
    }
    headers.push(("session_id".to_string(), resolved_session_id));

    if !input.strip_session_affinity {
        if input.include_turn_state {
            if let Some(turn_state) = input.incoming_turn_state {
                headers.push(("x-codex-turn-state".to_string(), turn_state.to_string()));
            }
        }
    }

    if input.include_account_id {
        if let Some(account_id) = input.account_id {
            headers.push(("ChatGPT-Account-ID".to_string(), account_id.to_string()));
        }
    }
    if should_forward_upstream_cookie() {
        if let Some(cookie) = input
            .upstream_cookie
            .filter(|value| !value.trim().is_empty())
        {
            headers.push(("Cookie".to_string(), cookie.to_string()));
        }
    }
    headers
}

pub(crate) fn build_codex_compact_upstream_headers(
    input: CodexCompactUpstreamHeaderInput<'_>,
) -> Vec<(String, String)> {
    let mut headers = Vec::with_capacity(8);
    headers.push((
        "Authorization".to_string(),
        format!("Bearer {}", input.auth_token),
    ));
    if input.has_body {
        headers.push(("Content-Type".to_string(), "application/json".to_string()));
    }
    headers.push(("Accept".to_string(), "application/json".to_string()));
    headers.push((
        "User-Agent".to_string(),
        crate::gateway::current_codex_user_agent(),
    ));
    headers.push((
        "originator".to_string(),
        crate::gateway::current_originator(),
    ));
    if let Some(residency_requirement) = crate::gateway::current_residency_requirement() {
        headers.push((
            crate::gateway::runtime_config::RESIDENCY_HEADER_NAME.to_string(),
            residency_requirement,
        ));
    }
    if let Some(subagent) = input
        .incoming_subagent
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        headers.push(("x-openai-subagent".to_string(), subagent.to_string()));
    }
    if let Some(session_id) = resolve_optional_session_id(
        input.incoming_session_id,
        input.fallback_session_id,
        input.strip_session_affinity,
    ) {
        headers.push(("session_id".to_string(), session_id));
    }
    if input.include_account_id {
        if let Some(account_id) = input.account_id {
            headers.push(("ChatGPT-Account-ID".to_string(), account_id.to_string()));
        }
    }
    let _ = input.upstream_cookie;
    headers
}

fn should_forward_upstream_cookie() -> bool {
    !crate::gateway::cpa_no_cookie_header_mode_enabled()
}

fn resolve_session_id(
    incoming: Option<&str>,
    fallback_session_id: Option<&str>,
    strip_session_affinity: bool,
) -> String {
    if strip_session_affinity {
        return random_session_id();
    }
    if let Some(value) = incoming {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Some(value) = fallback_session_id {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    random_session_id()
}

fn resolve_optional_session_id(
    incoming: Option<&str>,
    fallback_session_id: Option<&str>,
    strip_session_affinity: bool,
) -> Option<String> {
    if strip_session_affinity {
        return None;
    }
    if let Some(value) = incoming {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    fallback_session_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn resolve_client_request_id(incoming_client_request_id: Option<&str>) -> Option<String> {
    if let Some(value) = incoming_client_request_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(value.to_string());
    }
    None
}
