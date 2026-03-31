use super::*;

#[test]
fn strict_bearer_parsing_matches_auth_extraction_behavior() {
    assert_eq!(strict_bearer_token("Bearer abc"), Some("abc".to_string()));
    assert_eq!(strict_bearer_token("bearer abc"), None);
    assert_eq!(strict_bearer_token("Bearer   "), None);
}

#[test]
fn case_insensitive_bearer_parsing_matches_sticky_derivation_behavior() {
    assert_eq!(
        case_insensitive_bearer_token("Bearer abc"),
        Some("abc".to_string())
    );
    assert_eq!(
        case_insensitive_bearer_token("bearer abc"),
        Some("abc".to_string())
    );
    assert_eq!(case_insensitive_bearer_token("basic abc"), None);
    assert_eq!(case_insensitive_bearer_token("bearer   "), None);
}

#[test]
fn cli_affinity_override_preserves_existing_value_when_override_is_none() {
    let snapshot = IncomingHeaderSnapshot {
        cli_affinity_id: Some("cli-original".to_string()),
        ..IncomingHeaderSnapshot::default()
    };

    let overridden = snapshot.with_cli_affinity_id_override(None);

    assert_eq!(overridden.cli_affinity_id(), Some("cli-original"));
}

#[test]
fn cli_affinity_override_replaces_existing_value_when_override_is_present() {
    let snapshot = IncomingHeaderSnapshot {
        cli_affinity_id: Some("cli-original".to_string()),
        ..IncomingHeaderSnapshot::default()
    };

    let overridden = snapshot.with_cli_affinity_id_override(Some("cli-child"));

    assert_eq!(overridden.cli_affinity_id(), Some("cli-child"));
}
