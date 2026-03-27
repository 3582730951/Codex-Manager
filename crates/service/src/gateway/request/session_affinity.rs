#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutgoingSessionAffinity<'a> {
    pub(crate) incoming_session_id: Option<&'a str>,
    pub(crate) incoming_client_request_id: Option<&'a str>,
    pub(crate) incoming_turn_state: Option<&'a str>,
    pub(crate) fallback_session_id: Option<&'a str>,
}

fn normalize_anchor(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
pub(crate) fn has_thread_anchor_conflict(
    conversation_id: Option<&str>,
    prompt_cache_key: Option<&str>,
) -> bool {
    match (
        normalize_anchor(conversation_id),
        normalize_anchor(prompt_cache_key),
    ) {
        (Some(conversation_id), Some(prompt_cache_key)) => conversation_id != prompt_cache_key,
        _ => false,
    }
}

pub(crate) fn log_thread_anchor_conflict(
    context: &str,
    account_id: Option<&str>,
    conversation_id: Option<&str>,
    prompt_cache_key: Option<&str>,
) {
    let Some(conversation_id) = normalize_anchor(conversation_id) else {
        return;
    };
    let Some(prompt_cache_key) = normalize_anchor(prompt_cache_key) else {
        return;
    };
    if conversation_id == prompt_cache_key {
        return;
    }

    log::warn!(
        "event=gateway_thread_anchor_conflict context={} account_id={} conversation_fp={} prompt_cache_key_fp={} effective_source=prompt_cache_key",
        context,
        account_id.unwrap_or("-"),
        super::anchor_fingerprint::fingerprint_anchor(conversation_id),
        super::anchor_fingerprint::fingerprint_anchor(prompt_cache_key),
    );
}

pub(crate) fn derive_outgoing_session_affinity<'a>(
    incoming_session_id: Option<&'a str>,
    incoming_client_request_id: Option<&'a str>,
    incoming_turn_state: Option<&'a str>,
    conversation_id: Option<&'a str>,
    prompt_cache_key: Option<&'a str>,
) -> OutgoingSessionAffinity<'a> {
    let original_incoming_session_id = incoming_session_id;
    let mut resolved_turn_state = incoming_turn_state;
    let conversation_anchor = normalize_anchor(conversation_id);
    let effective_thread_anchor = normalize_anchor(prompt_cache_key).or(conversation_anchor);
    let resolved_client_request_id = conversation_anchor.or(incoming_client_request_id);
    let resolved_incoming_session_id = conversation_anchor.or(original_incoming_session_id);

    if resolved_turn_state.is_some()
        && original_incoming_session_id.is_none()
        && effective_thread_anchor.is_none()
    {
        // 中文注释：没有任何稳定线程锚点时，孤儿 turn-state 不应继续透传。
        resolved_turn_state = None;
    }
    if let (Some(thread_anchor), Some(conversation_anchor)) =
        (effective_thread_anchor, conversation_anchor)
    {
        if conversation_anchor != thread_anchor {
            // 中文注释：线程锚点与 conversation_id 冲突时，旧 turn-state 只能清掉。
            resolved_turn_state = None;
        }
    }

    OutgoingSessionAffinity {
        incoming_session_id: resolved_incoming_session_id,
        incoming_client_request_id: resolved_client_request_id,
        incoming_turn_state: resolved_turn_state,
        fallback_session_id: effective_thread_anchor,
    }
}

#[cfg(test)]
mod tests {
    use super::{derive_outgoing_session_affinity, has_thread_anchor_conflict};

    #[test]
    fn uses_conversation_anchor_when_prompt_cache_missing() {
        let actual = derive_outgoing_session_affinity(
            Some("legacy_session_should_not_win"),
            Some("legacy_request_id_should_not_win"),
            Some("legacy_turn_state_should_not_win"),
            Some("conv_anchor_only"),
            None,
        );

        assert_eq!(actual.incoming_session_id, Some("conv_anchor_only"));
        assert_eq!(actual.incoming_client_request_id, Some("conv_anchor_only"));
        assert_eq!(
            actual.incoming_turn_state,
            Some("legacy_turn_state_should_not_win")
        );
        assert_eq!(actual.fallback_session_id, Some("conv_anchor_only"));
    }

    #[test]
    fn uses_thread_anchor_for_fallback_headers() {
        let actual = derive_outgoing_session_affinity(
            Some("legacy_session_should_not_win"),
            Some("legacy_request_id_should_not_win"),
            Some("legacy_turn_state_should_not_win"),
            Some("conv_anchor_fallback"),
            Some("conv_anchor_fallback"),
        );

        assert_eq!(actual.incoming_session_id, Some("conv_anchor_fallback"));
        assert_eq!(
            actual.incoming_client_request_id,
            Some("conv_anchor_fallback")
        );
        assert_eq!(
            actual.incoming_turn_state,
            Some("legacy_turn_state_should_not_win")
        );
        assert_eq!(actual.fallback_session_id, Some("conv_anchor_fallback"));
    }

    #[test]
    fn clears_turn_state_when_thread_anchor_diverges() {
        let actual = derive_outgoing_session_affinity(
            Some("legacy_session_should_not_win"),
            Some("legacy_request_id_should_not_win"),
            Some("legacy_turn_state_should_not_win"),
            Some("conversation_anchor"),
            Some("prompt_thread_anchor"),
        );

        assert_eq!(actual.incoming_session_id, Some("conversation_anchor"));
        assert_eq!(
            actual.incoming_client_request_id,
            Some("conversation_anchor")
        );
        assert_eq!(actual.incoming_turn_state, None);
        assert_eq!(actual.fallback_session_id, Some("prompt_thread_anchor"));
    }

    #[test]
    fn drops_orphan_turn_state_without_conversation_anchor() {
        let actual = derive_outgoing_session_affinity(
            None,
            Some("explicit_client_request_id"),
            Some("turn_state_ok"),
            None,
            None,
        );

        assert_eq!(actual.incoming_session_id, None);
        assert_eq!(
            actual.incoming_client_request_id,
            Some("explicit_client_request_id")
        );
        assert_eq!(actual.incoming_turn_state, None);
        assert_eq!(actual.fallback_session_id, None);
    }

    #[test]
    fn conflict_detection_matches_anchor_mismatch() {
        assert!(has_thread_anchor_conflict(
            Some("conversation_anchor"),
            Some("prompt_thread_anchor")
        ));
        assert!(!has_thread_anchor_conflict(
            Some("conversation_anchor"),
            Some("conversation_anchor")
        ));
        assert!(!has_thread_anchor_conflict(
            Some("conversation_anchor"),
            None
        ));
    }
}
