use tiny_http::Request;

#[derive(Clone, Default)]
pub(crate) struct IncomingHeaderSnapshot {
    authorization_present: bool,
    x_api_key_present: bool,
    authorization_bearer_strict: Option<String>,
    authorization_bearer_case_insensitive: Option<String>,
    x_api_key: Option<String>,
    cli_affinity_id: Option<String>,
    session_id: Option<String>,
    client_request_id: Option<String>,
    subagent: Option<String>,
    beta_features: Option<String>,
    turn_metadata: Option<String>,
    turn_state: Option<String>,
    conversation_id: Option<String>,
    internal_client_entity: Option<String>,
    internal_peer_ip: Option<String>,
    internal_hop_sig: Option<String>,
}

impl IncomingHeaderSnapshot {
    pub(crate) fn from_request(request: &Request) -> Self {
        let mut snapshot = IncomingHeaderSnapshot::default();
        for header in request.headers() {
            if header.field.equiv("Authorization") {
                snapshot.authorization_present = true;
                let value = header.value.as_str().trim();
                if snapshot.authorization_bearer_strict.is_none() {
                    snapshot.authorization_bearer_strict = strict_bearer_token(value);
                }
                if snapshot.authorization_bearer_case_insensitive.is_none() {
                    snapshot.authorization_bearer_case_insensitive =
                        case_insensitive_bearer_token(value);
                }
                continue;
            }
            if header.field.equiv("x-api-key") {
                snapshot.x_api_key_present = true;
                if snapshot.x_api_key.is_none() {
                    let value = header.value.as_str().trim();
                    if !value.is_empty() {
                        snapshot.x_api_key = Some(value.to_string());
                    }
                }
                continue;
            }
            if snapshot.session_id.is_none() && header.field.equiv("session_id") {
                let value = header.value.as_str().trim();
                if !value.is_empty() {
                    snapshot.session_id = Some(value.to_string());
                }
                continue;
            }
            if snapshot.cli_affinity_id.is_none()
                && header.field.equiv("x-codex-cli-affinity-id")
            {
                let value = header.value.as_str().trim();
                if !value.is_empty() {
                    snapshot.cli_affinity_id = Some(value.to_string());
                }
                continue;
            }
            if snapshot.client_request_id.is_none() && header.field.equiv("x-client-request-id") {
                let value = header.value.as_str().trim();
                if !value.is_empty() {
                    snapshot.client_request_id = Some(value.to_string());
                }
                continue;
            }
            if snapshot.subagent.is_none() && header.field.equiv("x-openai-subagent") {
                let value = header.value.as_str().trim();
                if !value.is_empty() {
                    snapshot.subagent = Some(value.to_string());
                }
                continue;
            }
            if snapshot.beta_features.is_none() && header.field.equiv("x-codex-beta-features") {
                let value = header.value.as_str().trim();
                if !value.is_empty() {
                    snapshot.beta_features = Some(value.to_string());
                }
                continue;
            }
            if snapshot.turn_metadata.is_none() && header.field.equiv("x-codex-turn-metadata") {
                let value = header.value.as_str().trim();
                if !value.is_empty() {
                    snapshot.turn_metadata = Some(value.to_string());
                }
                continue;
            }
            if snapshot.turn_state.is_none() && header.field.equiv("x-codex-turn-state") {
                let value = header.value.as_str().trim();
                if !value.is_empty() {
                    snapshot.turn_state = Some(value.to_string());
                }
                continue;
            }
            if snapshot.conversation_id.is_none() && header.field.equiv("conversation_id") {
                let value = header.value.as_str().trim();
                if !value.is_empty() {
                    snapshot.conversation_id = Some(value.to_string());
                }
                continue;
            }
            if snapshot.internal_client_entity.is_none()
                && header
                    .field
                    .equiv(crate::gateway::affinity::INTERNAL_CLIENT_ENTITY_HEADER)
            {
                let value = header.value.as_str().trim();
                if !value.is_empty() {
                    snapshot.internal_client_entity = Some(value.to_string());
                }
                continue;
            }
            if snapshot.internal_peer_ip.is_none()
                && header
                    .field
                    .equiv(crate::gateway::affinity::INTERNAL_ENTITY_PEER_IP_HEADER)
            {
                let value = header.value.as_str().trim();
                if !value.is_empty() {
                    snapshot.internal_peer_ip = Some(value.to_string());
                }
                continue;
            }
            if snapshot.internal_hop_sig.is_none()
                && header
                    .field
                    .equiv(crate::gateway::affinity::INTERNAL_HOP_SIG_HEADER)
            {
                let value = header.value.as_str().trim();
                if !value.is_empty() {
                    snapshot.internal_hop_sig = Some(value.to_string());
                }
            }
        }
        snapshot
    }

    pub(crate) fn platform_key(&self) -> Option<&str> {
        self.x_api_key
            .as_deref()
            .or(self.authorization_bearer_strict.as_deref())
    }

    pub(crate) fn sticky_key_material(&self) -> Option<&str> {
        self.x_api_key
            .as_deref()
            .or(self.authorization_bearer_case_insensitive.as_deref())
    }

    pub(crate) fn has_authorization(&self) -> bool {
        self.authorization_present
    }

    pub(crate) fn has_x_api_key(&self) -> bool {
        self.x_api_key_present
    }

    pub(crate) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub(crate) fn cli_affinity_id(&self) -> Option<&str> {
        self.cli_affinity_id.as_deref()
    }

    pub(crate) fn client_request_id(&self) -> Option<&str> {
        self.client_request_id.as_deref()
    }

    pub(crate) fn subagent(&self) -> Option<&str> {
        self.subagent.as_deref()
    }

    pub(crate) fn beta_features(&self) -> Option<&str> {
        self.beta_features.as_deref()
    }

    pub(crate) fn turn_metadata(&self) -> Option<&str> {
        self.turn_metadata.as_deref()
    }

    pub(crate) fn turn_state(&self) -> Option<&str> {
        self.turn_state.as_deref()
    }

    pub(crate) fn conversation_id(&self) -> Option<&str> {
        self.conversation_id.as_deref()
    }

    pub(crate) fn internal_client_entity(&self) -> Option<&str> {
        self.internal_client_entity.as_deref()
    }

    pub(crate) fn internal_peer_ip(&self) -> Option<&str> {
        self.internal_peer_ip.as_deref()
    }

    pub(crate) fn internal_hop_sig(&self) -> Option<&str> {
        self.internal_hop_sig.as_deref()
    }

    pub(crate) fn with_conversation_id_override(&self, conversation_id: Option<&str>) -> Self {
        self.with_thread_affinity_override(conversation_id, false)
    }

    pub(crate) fn with_thread_affinity_override(
        &self,
        conversation_id: Option<&str>,
        reset_session_affinity: bool,
    ) -> Self {
        let mut next = self.clone();
        next.conversation_id = conversation_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if reset_session_affinity {
            next.session_id = None;
            next.turn_state = None;
        }
        next
    }
}

fn strict_bearer_token(value: &str) -> Option<String> {
    let token = value.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn case_insensitive_bearer_token(value: &str) -> Option<String> {
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

#[cfg(test)]
#[path = "tests/incoming_headers_tests.rs"]
mod tests;
