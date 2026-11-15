use trogon_credential_domain::{
    CredentialDestroyReason, CredentialFailureReason, CredentialFingerprint, CredentialIdentity, CredentialKind,
    CredentialMetadata, CredentialOwnerId, CredentialRef, CredentialScope, CredentialStatus, CredentialVersion,
    SourceKind,
};
use trogonai_proto::gateway::credentials::CredentialEventCase;

use super::*;
use crate::proto::{
    activated_to_proto, destroy_failed_to_proto, destroy_requested_to_proto, destroyed_to_proto, revoked_to_proto,
    rotated_to_proto, rotation_failed_to_proto, rotation_requested_to_proto, write_failed_to_proto,
    write_requested_to_proto,
};

fn identity() -> CredentialIdentity {
    CredentialIdentity::derive(
        CredentialScope::source(CredentialOwnerId::new("tenant-1").unwrap(), SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    )
}

fn credential_ref(value: u64) -> CredentialRef {
    CredentialRef::new(identity(), CredentialVersion::new(value).unwrap())
}

fn metadata(value: u64) -> CredentialMetadata {
    CredentialMetadata::new(
        credential_ref(value),
        CredentialStatus::Active,
        CredentialFingerprint::new("fingerprint").unwrap(),
    )
}

fn failure() -> CredentialFailureReason {
    CredentialFailureReason::new("openbao failed").unwrap()
}

fn case(event: impl Into<CredentialEventCase>) -> CredentialEventCase {
    event.into()
}

#[test]
fn a_write_that_has_not_landed_invalidates_nothing() {
    assert_eq!(invalidation_reason(&case(write_requested_to_proto(&identity()))), None);
    assert_eq!(
        invalidation_reason(&case(write_failed_to_proto(&identity().key(), &failure()))),
        None
    );
}

#[test]
fn an_activation_invalidates_nothing_since_nothing_released_before_it() {
    assert_eq!(invalidation_reason(&case(activated_to_proto(&metadata(1)))), None);
}

#[test]
fn a_requested_rotation_invalidates_nothing_since_it_still_releases_the_version_it_replaces() {
    assert_eq!(
        invalidation_reason(&case(rotation_requested_to_proto(&credential_ref(1)))),
        None
    );
}

#[test]
fn a_failed_rotation_invalidates_nothing_since_the_version_it_kept_still_releases() {
    assert_eq!(
        invalidation_reason(&case(rotation_failed_to_proto(&credential_ref(1), &failure()))),
        None
    );
}

#[test]
fn a_failed_destruction_invalidates_nothing_since_its_request_already_did() {
    assert_eq!(
        invalidation_reason(&case(destroy_failed_to_proto(&credential_ref(1), &failure()))),
        None
    );
}

#[test]
fn a_completed_rotation_invalidates_the_version_it_replaced() {
    assert_eq!(
        invalidation_reason(&case(rotated_to_proto(&credential_ref(1), &metadata(2)))),
        Some(InvalidationReason::Rotated)
    );
}

#[test]
fn a_revocation_invalidates_the_version_it_revoked() {
    assert_eq!(
        invalidation_reason(&case(revoked_to_proto(&credential_ref(1)))),
        Some(InvalidationReason::Revoked)
    );
}

#[test]
fn a_destruction_request_repeats_the_invalidation_for_a_consumer_that_missed_the_revocation() {
    let reason = CredentialDestroyReason::new("cleanup").unwrap();

    assert_eq!(
        invalidation_reason(&case(destroy_requested_to_proto(&credential_ref(1), &reason))),
        Some(InvalidationReason::Destroyed)
    );
}

#[test]
fn a_completed_destruction_repeats_the_invalidation_for_a_consumer_that_missed_the_revocation() {
    assert_eq!(
        invalidation_reason(&case(destroyed_to_proto(&credential_ref(1)))),
        Some(InvalidationReason::Destroyed)
    );
}
