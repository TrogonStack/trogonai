use std::time::{Duration, SystemTime};

use async_nats::HeaderMap;
use async_nats::jetstream::message::{OutboundMessage, StreamMessage};
use trogon_credential_aggregate::{InvalidationReason, credential_event_subject};
use trogon_credential_domain::{CredentialIdentity, CredentialKind, CredentialOwnerId, CredentialScope, SourceKind};
use trogon_decider_nats::TROGON_EVENT_TYPE;
use trogon_decider_runtime::{EventEncode, EventType, StreamPosition};
use trogonai_proto::gateway::credentials::{CredentialEventCase, v1};

use super::InvalidationOutcome;
use crate::fixtures::credential_events::{
    activated, destroy_requested, destroyed, message, raw_message, revoked, rotated, write_requested,
};
use crate::fixtures::{OWNER, identity, tenant};
use crate::{InvalidationNotice, InvalidationSourceError, StaticTenantOwners, TenantAccount};

const SEQUENCE: u64 = 42;
const STREAM: &str = "CREDENTIAL_EVENTS";
const APPENDED_SECONDS: i64 = 1_700_000_000;

/// `message` as the credential stream stores it, at [`SEQUENCE`].
fn stored(message: OutboundMessage) -> StreamMessage {
    StreamMessage {
        subject: message.subject,
        sequence: SEQUENCE,
        headers: message.headers.unwrap_or_default(),
        payload: message.payload,
        time: time::OffsetDateTime::from_unix_timestamp(APPENDED_SECONDS).unwrap(),
    }
}

fn holding(tenant: &TenantAccount, owner: &str) -> StaticTenantOwners {
    StaticTenantOwners::new().with_owner(tenant.clone(), CredentialOwnerId::new(owner).unwrap())
}

fn other_identity() -> CredentialIdentity {
    CredentialIdentity::derive(
        CredentialScope::source(CredentialOwnerId::new("project-2").unwrap(), SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    )
}

fn outcome(message: StreamMessage) -> Result<InvalidationOutcome, InvalidationSourceError> {
    InvalidationOutcome::of(STREAM, message, &holding(&tenant(), OWNER))
}

#[test]
fn a_message_without_an_event_id_is_not_a_recorded_event() {
    let event = revoked(&identity());
    let mut message = stored(message(&identity(), &event));
    let mut headers = HeaderMap::new();
    headers.insert(TROGON_EVENT_TYPE, event.event_type().unwrap());
    message.headers = headers;

    assert!(matches!(outcome(message), Err(InvalidationSourceError::Unrecorded(_))));
}

#[test]
fn a_message_without_an_event_type_is_not_a_recorded_event() {
    let message = stored(raw_message(&identity(), None, revoked(&identity()).encode().unwrap()));

    assert!(matches!(outcome(message), Err(InvalidationSourceError::Unrecorded(_))));
}

#[test]
fn a_message_at_sequence_zero_is_not_a_recorded_event() {
    let mut message = stored(message(&identity(), &revoked(&identity())));
    message.sequence = 0;

    assert!(matches!(outcome(message), Err(InvalidationSourceError::Unrecorded(_))));
}

#[test]
fn an_event_type_this_build_does_not_know_is_reported_rather_than_skipped() {
    let message = stored(raw_message(&identity(), Some("acme.v1.Unknown"), Vec::new()));

    match outcome(message) {
        Err(InvalidationSourceError::UnknownEventType { event_type }) => assert_eq!(event_type, "acme.v1.Unknown"),
        other => panic!("expected an unknown event type, got {other:?}"),
    }
}

#[test]
fn a_payload_that_does_not_decode_is_reported() {
    let event_type = revoked(&identity()).event_type().unwrap();
    let message = stored(raw_message(&identity(), Some(event_type), vec![0xff; 4]));

    assert!(matches!(outcome(message), Err(InvalidationSourceError::Undecodable(_))));
}

#[test]
fn an_invalidating_event_that_names_no_credential_is_reported() {
    let event = v1::CredentialEvent {
        event: Some(CredentialEventCase::Revoked(Box::default())),
    };
    let message = stored(message(&identity(), &event));

    assert!(matches!(
        outcome(message),
        Err(InvalidationSourceError::InvalidEvent(_))
    ));
}

#[test]
fn an_invalidating_event_appended_on_another_credentials_subject_is_reported() {
    let event = revoked(&identity());
    let message = stored(raw_message(
        &other_identity(),
        Some(event.event_type().unwrap()),
        event.encode().unwrap(),
    ));
    let other_subject = credential_event_subject(&other_identity().key()).unwrap();

    match outcome(message) {
        Err(InvalidationSourceError::EventFromAnotherSubject { credential, subject }) => {
            assert_eq!(credential, identity().key());
            assert_eq!(subject, other_subject.as_str());
        }
        other => panic!("expected an event from another subject, got {other:?}"),
    }
}

#[test]
fn an_invalidating_event_appended_at_the_unix_epoch_is_reported() {
    let mut message = stored(message(&identity(), &revoked(&identity())));
    message.time = time::OffsetDateTime::UNIX_EPOCH;

    assert!(matches!(
        outcome(message),
        Err(InvalidationSourceError::NotAppendedAfterEpoch { sequence: SEQUENCE })
    ));
}

#[test]
fn an_invalidating_event_whose_owner_no_tenant_holds_has_no_notice() {
    let message = stored(message(&identity(), &revoked(&identity())));

    let outcome = InvalidationOutcome::of(STREAM, message, &holding(&tenant(), "project-2")).unwrap();

    assert_eq!(
        outcome,
        InvalidationOutcome::OwnerHeldByNoTenant {
            credential: identity().key()
        }
    );
}

#[test]
fn an_event_that_leaves_the_release_as_it_was_publishes_nothing() {
    for event in [write_requested(&identity()), activated(&identity())] {
        let message = stored(message(&identity(), &event));

        assert_eq!(outcome(message).unwrap(), InvalidationOutcome::NothingToPublish);
    }
}

#[test]
fn an_event_that_leaves_the_release_as_it_was_publishes_nothing_even_when_no_tenant_holds_its_owner() {
    let message = stored(message(&identity(), &activated(&identity())));

    let outcome = InvalidationOutcome::of(STREAM, message, &StaticTenantOwners::new()).unwrap();

    assert_eq!(outcome, InvalidationOutcome::NothingToPublish);
}

#[test]
fn each_invalidating_event_publishes_its_reason_to_the_tenant_holding_the_owner() {
    let tenant = tenant();
    let cases = [
        (rotated(&identity()), InvalidationReason::Rotated),
        (revoked(&identity()), InvalidationReason::Revoked),
        (destroy_requested(&identity()), InvalidationReason::Destroyed),
        (destroyed(&identity()), InvalidationReason::Destroyed),
    ];

    for (event, reason) in cases {
        let message = stored(message(&identity(), &event));

        let outcome = InvalidationOutcome::of(STREAM, message, &holding(&tenant, OWNER)).unwrap();

        let expected = InvalidationNotice::new(
            &tenant,
            &identity().key(),
            reason,
            STREAM,
            SystemTime::UNIX_EPOCH + Duration::from_secs(u64::try_from(APPENDED_SECONDS).unwrap()),
            StreamPosition::try_new(SEQUENCE).unwrap(),
        )
        .unwrap();
        assert_eq!(outcome, InvalidationOutcome::Publish(expected), "{reason:?}");
    }
}
