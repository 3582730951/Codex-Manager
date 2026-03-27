#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PromptCacheKeyRewriteSource {
    MissingInput,
    PreservedExisting,
    Inserted,
    ForcedReused,
    ForcedReplaced,
}

impl PromptCacheKeyRewriteSource {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::MissingInput => "missing_input",
            Self::PreservedExisting => "preserved_existing",
            Self::Inserted => "inserted",
            Self::ForcedReused => "forced_reused",
            Self::ForcedReplaced => "forced_replaced",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PromptCacheKeyRewriteDecision<'a> {
    pub(super) source: PromptCacheKeyRewriteSource,
    pub(super) final_value: Option<&'a str>,
    pub(super) changed: bool,
}

pub(super) fn resolve_prompt_cache_key_rewrite<'a>(
    existing: Option<&'a str>,
    prompt_cache_key: Option<&'a str>,
    force_override: bool,
) -> PromptCacheKeyRewriteDecision<'a> {
    let existing = existing.map(str::trim).filter(|value| !value.is_empty());
    let requested = prompt_cache_key
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match requested {
        None => PromptCacheKeyRewriteDecision {
            source: PromptCacheKeyRewriteSource::MissingInput,
            final_value: existing,
            changed: false,
        },
        Some(requested) => match existing {
            Some(existing) if !force_override => PromptCacheKeyRewriteDecision {
                source: PromptCacheKeyRewriteSource::PreservedExisting,
                final_value: Some(existing),
                changed: false,
            },
            Some(existing) if existing == requested => PromptCacheKeyRewriteDecision {
                source: PromptCacheKeyRewriteSource::ForcedReused,
                final_value: Some(existing),
                changed: false,
            },
            Some(_) => PromptCacheKeyRewriteDecision {
                source: PromptCacheKeyRewriteSource::ForcedReplaced,
                final_value: Some(requested),
                changed: true,
            },
            None => PromptCacheKeyRewriteDecision {
                source: PromptCacheKeyRewriteSource::Inserted,
                final_value: Some(requested),
                changed: true,
            },
        },
    }
}

pub(super) fn fingerprint_prompt_cache_key(value: &str) -> String {
    super::super::anchor_fingerprint::fingerprint_anchor(value)
}

#[cfg(test)]
mod tests {
    use super::{
        fingerprint_prompt_cache_key, resolve_prompt_cache_key_rewrite, PromptCacheKeyRewriteSource,
    };

    #[test]
    fn missing_input_keeps_existing_value() {
        let actual = resolve_prompt_cache_key_rewrite(Some("existing"), None, false);

        assert_eq!(actual.source, PromptCacheKeyRewriteSource::MissingInput);
        assert_eq!(actual.final_value, Some("existing"));
        assert!(!actual.changed);
    }

    #[test]
    fn inserted_prompt_cache_key_is_reported() {
        let actual = resolve_prompt_cache_key_rewrite(None, Some("thread_1"), false);

        assert_eq!(actual.source, PromptCacheKeyRewriteSource::Inserted);
        assert_eq!(actual.final_value, Some("thread_1"));
        assert!(actual.changed);
    }

    #[test]
    fn forced_prompt_cache_key_replaces_existing_value() {
        let actual = resolve_prompt_cache_key_rewrite(Some("old"), Some("new"), true);

        assert_eq!(actual.source, PromptCacheKeyRewriteSource::ForcedReplaced);
        assert_eq!(actual.final_value, Some("new"));
        assert!(actual.changed);
    }

    #[test]
    fn fingerprint_is_stable() {
        assert_eq!(fingerprint_prompt_cache_key("thread_1").len(), 16);
        assert_eq!(
            fingerprint_prompt_cache_key("thread_1"),
            fingerprint_prompt_cache_key("thread_1")
        );
    }
}
