use super::*;
use crate::credential::commands::domain::{CredentialOwnerId, SourceKind};
use crate::source_integration_id::SourceIntegrationId;

fn owner(value: &str) -> CredentialOwnerId {
    CredentialOwnerId::new(value).unwrap()
}

#[test]
fn derivation_is_stable_for_the_same_slot() {
    let scope = CredentialScope::source(owner("tenant-1"), SourceKind::GitHub);

    assert_eq!(
        CredentialId::derive(&scope, CredentialKind::WebhookSecret),
        CredentialId::derive(&scope, CredentialKind::WebhookSecret)
    );
}

#[test]
fn derivation_is_pinned_so_a_namespace_or_template_change_cannot_pass_unnoticed() {
    let source = CredentialScope::source(owner("tenant-1"), SourceKind::GitHub);
    let integration = CredentialScope::integration(
        owner("tenant-1"),
        SourceKind::GitHub,
        SourceIntegrationId::new("primary").unwrap(),
    );

    assert_eq!(
        CredentialId::derive(&source, CredentialKind::WebhookSecret).to_string(),
        "93171e83-3fe8-5776-ab3e-f27b7fdaed81"
    );
    assert_eq!(
        CredentialId::derive(&integration, CredentialKind::WebhookSecret).to_string(),
        "9d797971-11ef-5d18-8bde-61c54768625b"
    );
}

#[test]
fn derivation_hashes_the_slash_separated_template() {
    let scope = CredentialScope::integration(
        owner("tenant-1"),
        SourceKind::GitHub,
        SourceIntegrationId::new("primary").unwrap(),
    );

    assert_eq!(
        derivation_name(&scope, CredentialKind::WebhookSecret),
        "tenant-1/github/primary/webhook_secret"
    );
}

#[test]
fn derivation_names_are_unambiguous() {
    for invalid in ["tenant/1", "tenant 1", "tenant\u{0}1"] {
        assert!(CredentialOwnerId::new(invalid).is_err(), "{invalid:?}");
    }
    assert!(SourceIntegrationId::new("primary/secondary").is_err());
    for source in SourceKind::ALL {
        assert!(source.as_str().chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'));
    }
    for kind in CredentialKind::ALL {
        assert!(kind.as_str().chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'));
    }

    let owner_id = owner("Tenant-1_a.b:c");
    let scope = CredentialScope::integration(
        owner_id.clone(),
        SourceKind::MicrosoftGraph,
        SourceIntegrationId::new("Primary-1_a").unwrap(),
    );
    let name = derivation_name(&scope, CredentialKind::ServiceAccountCredentials);
    let (name_owner, rest) = name.split_once('/').unwrap();
    let (scope_key, kind) = rest.rsplit_once('/').unwrap();

    assert_eq!(name_owner, owner_id.as_str());
    assert_eq!(scope_key, scope.scope_key());
    assert_eq!(kind, CredentialKind::ServiceAccountCredentials.as_str());
}

#[test]
fn derivation_separates_owner_scope_and_kind() {
    let integration = SourceIntegrationId::new("primary").unwrap();
    let base = CredentialId::derive(
        &CredentialScope::source(owner("tenant-1"), SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    );
    let other_owner = CredentialId::derive(
        &CredentialScope::source(owner("tenant-2"), SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    );
    let other_scope = CredentialId::derive(
        &CredentialScope::integration(owner("tenant-1"), SourceKind::GitHub, integration),
        CredentialKind::WebhookSecret,
    );
    let other_kind = CredentialId::derive(
        &CredentialScope::source(owner("tenant-1"), SourceKind::GitHub),
        CredentialKind::BotToken,
    );

    assert_ne!(base, other_owner);
    assert_ne!(base, other_scope);
    assert_ne!(base, other_kind);
}

#[test]
fn a_derived_id_names_neither_owner_scope_kind_nor_backend() {
    let scope = CredentialScope::source(owner("tenant-1"), SourceKind::GitHub);
    let id = CredentialId::derive(&scope, CredentialKind::WebhookSecret).to_string();

    for leaked in ["tenant", "github", "webhook", "openbao", ":", "/"] {
        assert!(!id.contains(leaked), "{leaked}");
    }
}

#[test]
fn a_derived_id_survives_a_round_trip_through_its_text() {
    let scope = CredentialScope::source(owner("tenant-1"), SourceKind::GitHub);
    let id = CredentialId::derive(&scope, CredentialKind::WebhookSecret);

    assert_eq!(CredentialId::new(id.to_string()), Ok(id));
}

#[test]
fn rejects_empty_id() {
    assert_eq!(CredentialId::new(""), Err(CredentialIdError::Empty));
}

#[test]
fn rejects_the_simple_form_the_previous_ids_used() {
    assert_eq!(
        CredentialId::new("63a3ec074aa850dc94f6b9a6078c28ad"),
        Err(CredentialIdError::InvalidLength(32))
    );
}

#[test]
fn rejects_anything_but_lowercase_hex_and_hyphens() {
    for (value, ch) in [
        ("93171E83-3fe8-5776-ab3e-f27b7fdaed81", 'E'),
        ("93171e83-3fe8-5776-ab3e-f27b7fdaed8g", 'g'),
        ("{3171e83-3fe8-5776-ab3e-f27b7fdaed8}", '{'),
    ] {
        assert_eq!(CredentialId::new(value), Err(CredentialIdError::InvalidCharacter(ch)));
    }
}

#[test]
fn rejects_hyphens_out_of_place() {
    let error = CredentialId::new("93171e833-fe8-5776-ab3e-f27b7fdaed81").unwrap_err();

    assert!(matches!(error, CredentialIdError::InvalidFormat(_)));
    let source = std::error::Error::source(&error).expect("the UUID parse error");
    assert_eq!(
        error.to_string(),
        format!("credential id is not a hyphenated UUID: {source}")
    );
}

#[test]
fn debug_names_the_type_and_value() {
    let id = CredentialId::new("93171e83-3fe8-5776-ab3e-f27b7fdaed81").unwrap();

    assert_eq!(
        format!("{id:?}"),
        "CredentialId(\"93171e83-3fe8-5776-ab3e-f27b7fdaed81\")"
    );
}
