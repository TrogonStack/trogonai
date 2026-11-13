use trogon_credential_domain::{CredentialIdentity, CredentialKind, CredentialScope, SourceKind};

use super::*;

fn owner(raw: &str) -> CredentialOwnerId {
    CredentialOwnerId::new(raw).unwrap()
}

fn stream_id(owner_id: &str) -> CredentialKey {
    CredentialIdentity::derive(
        CredentialScope::source(owner(owner_id), SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    )
    .key()
}

#[test]
fn subject_sits_below_the_project_scope_with_the_opaque_id_as_the_last_token() {
    let stream_id = stream_id("tenant-1");
    let subject = credential_event_subject(&stream_id).unwrap();

    assert_eq!(
        subject.as_str(),
        format!(
            "{CREDENTIAL_EVENT_SUBJECT_PREFIX}.tenant-1.{}",
            stream_id.credential_id()
        )
    );
}

#[test]
fn resolver_scope_is_the_project_subtree() {
    let resolver = CredentialEventSubjectResolver::for_owner(&owner("tenant-1")).unwrap();

    assert_eq!(
        resolver.scope().as_prefix(),
        format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.tenant-1.")
    );
    assert_eq!(resolver.subject_scope(), Some(resolver.scope()));
}

#[test]
fn dotted_project_ids_stay_a_single_subject_token() {
    let resolver = CredentialEventSubjectResolver::for_owner(&owner("acme.prod")).unwrap();

    assert_eq!(
        resolver.scope().as_prefix(),
        format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.acme~prod.")
    );
}

#[test]
fn a_dotted_project_cannot_reach_into_another_projects_subtree() {
    let dotted = credential_event_subject(&stream_id("acme.prod")).unwrap();
    let other = CredentialEventSubjectResolver::for_owner(&owner("acme")).unwrap();

    assert!(!dotted.as_str().starts_with(other.scope().as_prefix()));
}

#[test]
fn resolver_rejects_a_stream_id_from_another_project() {
    let resolver = CredentialEventSubjectResolver::for_owner(&owner("tenant-1")).unwrap();

    let error = resolver.subject_for(&stream_id("tenant-2")).unwrap_err();

    assert!(matches!(
        error,
        CredentialEventSubjectResolverError::OutsideProject { owner_id, .. } if owner_id.as_str() == "tenant-1"
    ));
}

#[test]
fn the_same_slot_in_two_projects_resolves_to_two_subjects() {
    let first = credential_event_subject(&stream_id("tenant-1")).unwrap();
    let second = credential_event_subject(&stream_id("tenant-2")).unwrap();

    assert_ne!(first.as_str(), second.as_str());
}
