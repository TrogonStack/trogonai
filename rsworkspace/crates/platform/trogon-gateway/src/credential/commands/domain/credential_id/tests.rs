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
fn derivation_is_pinned_so_a_namespace_change_cannot_pass_unnoticed() {
    let scope = CredentialScope::source(owner("tenant-1"), SourceKind::GitHub);

    assert_eq!(
        CredentialId::derive(&scope, CredentialKind::WebhookSecret).as_str(),
        "63a3ec074aa850dc94f6b9a6078c28ad"
    );
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
    let id = CredentialId::derive(&scope, CredentialKind::WebhookSecret);

    for leaked in ["tenant", "github", "webhook", "openbao", ":", "/"] {
        assert!(!id.as_str().contains(leaked), "{leaked}");
    }
}

#[test]
fn a_derived_id_survives_a_round_trip_through_its_text() {
    let scope = CredentialScope::source(owner("tenant-1"), SourceKind::GitHub);
    let id = CredentialId::derive(&scope, CredentialKind::WebhookSecret);

    assert_eq!(CredentialId::new(id.as_str()), Ok(id));
}

#[test]
fn rejects_empty_id() {
    assert_eq!(CredentialId::new(""), Err(CredentialIdError::Empty));
}

#[test]
fn rejects_a_parseable_legacy_id() {
    assert_eq!(
        CredentialId::new("openbao:tenant-1:github:webhook_secret"),
        Err(CredentialIdError::InvalidLength(38))
    );
}

#[test]
fn rejects_the_wrong_length() {
    assert_eq!(CredentialId::new("abcd"), Err(CredentialIdError::InvalidLength(4)));
}

#[test]
fn rejects_anything_but_lowercase_hex() {
    for (value, ch) in [
        ("ab6f9f8b4ffc5bd9b36bf3b0fa5e3dA4", 'A'),
        ("ab6f9f8b-ffc5bd9b36bf3b0fa5e3da4", '-'),
        ("ab6f9f8b4ffc5bd9b36bf3b0fa5e3dg4", 'g'),
    ] {
        assert_eq!(CredentialId::new(value), Err(CredentialIdError::InvalidCharacter(ch)));
    }
}

#[test]
fn debug_names_the_type_and_value() {
    let id = CredentialId::new("ab6f9f8b4ffc5bd9b36bf3b0fa5e3da4").unwrap();

    assert_eq!(format!("{id:?}"), "CredentialId(\"ab6f9f8b4ffc5bd9b36bf3b0fa5e3da4\")");
}
