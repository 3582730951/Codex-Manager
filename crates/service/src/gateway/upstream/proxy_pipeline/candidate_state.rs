use bytes::Bytes;
use codexmanager_core::storage::Account;
use std::collections::HashMap;

use super::super::support::payload_rewrite::strip_encrypted_content_from_body;
use super::request_setup::UpstreamRequestSetup;

#[derive(Default)]
pub(super) struct CandidateExecutionState {
    stripped_body: Option<Bytes>,
    rewritten_bodies: HashMap<String, Bytes>,
    stripped_rewritten_bodies: HashMap<String, Bytes>,
    first_candidate_account_scope: Option<String>,
}

impl CandidateExecutionState {
    pub(super) fn strip_session_affinity(
        &mut self,
        account: &Account,
        idx: usize,
        anthropic_has_prompt_cache_key: bool,
    ) -> bool {
        if !anthropic_has_prompt_cache_key {
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
        body: &Bytes,
        setup: &UpstreamRequestSetup,
        model_override: Option<&str>,
    ) -> Bytes {
        let Some(model_override) = model_override
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return body.clone();
        };

        self.rewritten_bodies
            .entry(model_override.to_string())
            .or_insert_with(|| {
                Bytes::from(super::super::super::apply_request_overrides(
                    path,
                    body.to_vec(),
                    Some(model_override),
                    None,
                    Some(setup.upstream_base.as_str()),
                ))
            })
            .clone()
    }

    pub(super) fn body_for_attempt(
        &mut self,
        path: &str,
        body: &Bytes,
        strip_session_affinity: bool,
        setup: &UpstreamRequestSetup,
        model_override: Option<&str>,
    ) -> Bytes {
        let rewritten = self.rewrite_body_for_model(path, body, setup, model_override);
        if strip_session_affinity && setup.has_body_encrypted_content {
            if let Some(model_override) = model_override
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return self
                    .stripped_rewritten_bodies
                    .entry(model_override.to_string())
                    .or_insert_with(|| {
                        strip_encrypted_content_from_body(rewritten.as_ref())
                            .map(Bytes::from)
                            .unwrap_or_else(|| rewritten.clone())
                    })
                    .clone();
            }
            if self.stripped_body.is_none() {
                self.stripped_body = strip_encrypted_content_from_body(rewritten.as_ref())
                    .map(Bytes::from)
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

    pub(super) fn retry_body(
        &mut self,
        path: &str,
        body: &Bytes,
        setup: &UpstreamRequestSetup,
        model_override: Option<&str>,
    ) -> Bytes {
        let rewritten = self.rewrite_body_for_model(path, body, setup, model_override);
        if setup.has_body_encrypted_content {
            if let Some(model_override) = model_override
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                return self
                    .stripped_rewritten_bodies
                    .entry(model_override.to_string())
                    .or_insert_with(|| {
                        strip_encrypted_content_from_body(rewritten.as_ref())
                            .map(Bytes::from)
                            .unwrap_or_else(|| rewritten.clone())
                    })
                    .clone();
            }
            if self.stripped_body.is_none() {
                self.stripped_body = strip_encrypted_content_from_body(rewritten.as_ref())
                    .map(Bytes::from)
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
    use bytes::Bytes;

    #[test]
    fn body_for_attempt_rewrites_model_override() {
        let mut state = CandidateExecutionState::default();
        let body = Bytes::from_static(br#"{"model":"gpt-5.4","input":"hello"}"#);
        let setup = super::super::request_setup::UpstreamRequestSetup {
            upstream_base: "https://chatgpt.com/backend-api/codex".to_string(),
            upstream_fallback_base: None,
            url: "https://chatgpt.com/backend-api/codex/responses".to_string(),
            url_alt: None,
            upstream_cookie: None,
            candidate_count: 1,
            account_max_inflight: 1,
            anthropic_has_prompt_cache_key: false,
            has_sticky_fallback_session: false,
            has_sticky_fallback_conversation: false,
            has_body_encrypted_content: false,
        };

        let actual = state.body_for_attempt("/v1/responses", &body, false, &setup, Some("gpt-5.2"));
        let value: serde_json::Value =
            serde_json::from_slice(actual.as_ref()).expect("parse rewritten body");

        assert_eq!(
            value.get("model").and_then(serde_json::Value::as_str),
            Some("gpt-5.2")
        );
    }
}
