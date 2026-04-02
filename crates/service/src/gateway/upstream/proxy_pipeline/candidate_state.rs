use codexmanager_core::storage::Account;
use std::collections::HashMap;

use super::super::support::payload_rewrite::strip_encrypted_content_from_payload;
use super::request_setup::UpstreamRequestSetup;

#[derive(Default)]
pub(in super::super) struct CandidateExecutionState {
    stripped_body: Option<crate::gateway::RequestPayload>,
    rewritten_bodies: HashMap<String, crate::gateway::RequestPayload>,
    stripped_rewritten_bodies: HashMap<String, crate::gateway::RequestPayload>,
    first_candidate_account_scope: Option<String>,
}

impl CandidateExecutionState {
    fn base_body_for_attempt<'a>(
        &self,
        body: &'a crate::gateway::RequestPayload,
        setup: &'a UpstreamRequestSetup,
        body_override: Option<&'a crate::gateway::RequestPayload>,
    ) -> &'a crate::gateway::RequestPayload {
        body_override
            .or(setup.request_body_override.as_ref())
            .unwrap_or(body)
    }

    fn rewrite_cache_key(
        model_override: Option<&str>,
        prompt_cache_key: Option<&str>,
    ) -> Option<String> {
        let normalized_model = model_override
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let normalized_prompt_cache_key = prompt_cache_key
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if normalized_model.is_none() && normalized_prompt_cache_key.is_none() {
            return None;
        }
        Some(format!(
            "model={}|thread={}",
            normalized_model.unwrap_or("-"),
            normalized_prompt_cache_key.unwrap_or("-")
        ))
    }

    pub(in super::super) fn strip_session_affinity(
        &mut self,
        account: &Account,
        idx: usize,
        has_prompt_cache_affinity: bool,
    ) -> bool {
        if !has_prompt_cache_affinity {
            return idx > 0;
        }
        let candidate_scope = account
            .chatgpt_account_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string())
            .or_else(|| {
                account
                    .workspace_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| value.to_string())
            });
        if idx == 0 {
            self.first_candidate_account_scope = candidate_scope.clone();
            false
        } else {
            candidate_scope != self.first_candidate_account_scope
        }
    }

    fn rewrite_body_for_model(
        &mut self,
        path: &str,
        body: &crate::gateway::RequestPayload,
        setup: &UpstreamRequestSetup,
        body_override: Option<&crate::gateway::RequestPayload>,
        model_override: Option<&str>,
        prompt_cache_key: Option<&str>,
    ) -> crate::gateway::RequestPayload {
        let base_body = self.base_body_for_attempt(body, setup, body_override);
        let Some(cache_key) = Self::rewrite_cache_key(model_override, prompt_cache_key) else {
            return base_body.clone();
        };

        self.rewritten_bodies
            .entry(cache_key)
            .or_insert_with(|| {
                super::super::super::apply_request_overrides_payload_with_service_tier_and_forced_prompt_cache_key(
                    path,
                    base_body,
                    model_override,
                    None,
                    None,
                    Some(setup.upstream_base.as_str()),
                    prompt_cache_key,
                )
                .expect("candidate payload rewrite should remain serializable")
            })
            .clone()
    }

    pub(in super::super) fn body_for_attempt(
        &mut self,
        path: &str,
        body: &crate::gateway::RequestPayload,
        strip_session_affinity: bool,
        setup: &UpstreamRequestSetup,
        body_override: Option<&crate::gateway::RequestPayload>,
        model_override: Option<&str>,
        prompt_cache_key: Option<&str>,
    ) -> crate::gateway::RequestPayload {
        let rewritten = self.rewrite_body_for_model(
            path,
            body,
            setup,
            body_override,
            model_override,
            prompt_cache_key,
        );
        if strip_session_affinity && setup.has_body_encrypted_content {
            if let Some(cache_key) = Self::rewrite_cache_key(model_override, prompt_cache_key) {
                return self
                    .stripped_rewritten_bodies
                    .entry(cache_key)
                    .or_insert_with(|| {
                        strip_encrypted_content_from_payload(&rewritten)
                            .expect("candidate stripped payload should remain readable")
                            .unwrap_or_else(|| rewritten.clone())
                    })
                    .clone();
            }
            if self.stripped_body.is_none() {
                self.stripped_body = strip_encrypted_content_from_payload(&rewritten)
                    .expect("candidate stripped payload should remain readable")
                    .or_else(|| Some(rewritten.clone()));
            }
            self.stripped_body
                .as_ref()
                .expect("stripped body should be initialized")
                .clone()
        } else {
            rewritten
        }
    }

    pub(in super::super) fn retry_body(
        &mut self,
        path: &str,
        body: &crate::gateway::RequestPayload,
        setup: &UpstreamRequestSetup,
        body_override: Option<&crate::gateway::RequestPayload>,
        model_override: Option<&str>,
        prompt_cache_key: Option<&str>,
    ) -> crate::gateway::RequestPayload {
        let rewritten = self.rewrite_body_for_model(
            path,
            body,
            setup,
            body_override,
            model_override,
            prompt_cache_key,
        );
        if setup.has_body_encrypted_content {
            if let Some(cache_key) = Self::rewrite_cache_key(model_override, prompt_cache_key) {
                return self
                    .stripped_rewritten_bodies
                    .entry(cache_key)
                    .or_insert_with(|| {
                        strip_encrypted_content_from_payload(&rewritten)
                            .expect("candidate stripped payload should remain readable")
                            .unwrap_or_else(|| rewritten.clone())
                    })
                    .clone();
            }
            if self.stripped_body.is_none() {
                self.stripped_body = strip_encrypted_content_from_payload(&rewritten)
                    .expect("candidate stripped payload should remain readable")
                    .or_else(|| Some(rewritten.clone()));
            }
            self.stripped_body
                .as_ref()
                .expect("stripped body should be initialized")
                .clone()
        } else {
            rewritten
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CandidateExecutionState;

    #[test]
    fn body_for_attempt_rewrites_model_override() {
        let mut state = CandidateExecutionState::default();
        let body = crate::gateway::RequestPayload::from_vec(
            br#"{"model":"gpt-5.4","input":"hello"}"#.to_vec(),
        )
        .expect("build request payload");
        let setup = super::super::request_setup::UpstreamRequestSetup {
            upstream_base: "https://chatgpt.com/backend-api/codex".to_string(),
            upstream_fallback_base: None,
            url: "https://chatgpt.com/backend-api/codex/responses".to_string(),
            url_alt: None,
            candidate_count: 1,
            account_max_inflight: 1,
            account_dynamic_limits: std::collections::HashMap::new(),
            anthropic_has_prompt_cache_key: false,
            has_sticky_fallback_session: false,
            has_sticky_fallback_conversation: false,
            has_body_encrypted_content: false,
            request_body_override: None,
            routing_state: super::super::request_setup::RequestRoutingState::StatelessNoLegacy,
            peer_runtime_key: None,
        };

        let actual = state.body_for_attempt(
            "/v1/responses",
            &body,
            false,
            &setup,
            None,
            Some("gpt-5.2"),
            Some("thread-2"),
        );
        let value = actual.read_json_value().expect("parse rewritten body");

        assert_eq!(
            value.get("model").and_then(serde_json::Value::as_str),
            Some("gpt-5.2")
        );
        assert_eq!(
            value
                .get("prompt_cache_key")
                .and_then(serde_json::Value::as_str),
            Some("thread-2")
        );
    }
}
