use bytes::Bytes;
use codexmanager_core::storage::Account;

use super::super::support::payload_rewrite::strip_encrypted_content_from_body;
use super::request_setup::UpstreamRequestSetup;

#[derive(Default)]
pub(super) struct CandidateExecutionState {
    stripped_body: Option<Bytes>,
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

    pub(super) fn body_for_attempt<'a>(
        &'a mut self,
        body: &'a Bytes,
        strip_session_affinity: bool,
        setup: &UpstreamRequestSetup,
    ) -> &'a Bytes {
        let should_strip_body_affinity = setup.has_body_encrypted_content
            && (strip_session_affinity || crate::gateway::cpa_no_cookie_header_mode_enabled());
        if should_strip_body_affinity {
            // 中文注释：请求头收敛模式会主动移除 turn-state / conversation_id。
            // 若此时仍把 body 里的 compaction/encrypted_content 原样上送，
            // upstream 更容易在长历史上报 `input[n] compat`。
            if self.stripped_body.is_none() {
                self.stripped_body = strip_encrypted_content_from_body(body.as_ref())
                    .map(Bytes::from)
                    .or_else(|| Some(body.clone()));
            }
            self.stripped_body
                .as_ref()
                .expect("stripped body should be initialized")
        } else {
            body
        }
    }

    pub(super) fn retry_body<'a>(
        &'a mut self,
        body: &'a Bytes,
        setup: &UpstreamRequestSetup,
    ) -> &'a Bytes {
        if setup.has_body_encrypted_content {
            if self.stripped_body.is_none() {
                self.stripped_body = strip_encrypted_content_from_body(body.as_ref())
                    .map(Bytes::from)
                    .or_else(|| Some(body.clone()));
            }
            self.stripped_body
                .as_ref()
                .expect("stripped body should be initialized")
        } else {
            body
        }
    }
}
