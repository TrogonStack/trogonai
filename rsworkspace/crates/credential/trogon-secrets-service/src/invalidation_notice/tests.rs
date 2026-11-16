use std::time::{Duration, SystemTime};

use buffa::Message as _;
use trogon_credential_aggregate::InvalidationReason;
use trogon_credential_domain::{CredentialId, CredentialKey, CredentialOwnerId};
use trogon_decider_runtime::StreamPosition;
use trogonai_proto::secrets::exchange::v1alpha1::{CredentialAddress, ReleaseInvalidated, ReleaseInvalidationReason};
use uuid::Uuid;

use super::InvalidationNotice;
use crate::constants::{INVALIDATION_MAX_MESSAGE_SIZE, INVALIDATION_SUBJECTS};
use crate::{InvalidationSourceError, TenantAccount};

/// A fixed account key, so a subject can be compared to a literal.
const TENANT: &str = "ADVEU3DD4KOFECV66VIHWEZOYX4ZKR3WV27L464SIIPOU2IUI3JCZW22";
const CREDENTIAL_ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
const APPENDED_SECONDS: u64 = 1_700_000_000;
const APPENDED_NANOS: u32 = 250;
const SEQUENCE: u64 = 42;
const REASONS: [InvalidationReason; 3] = [
    InvalidationReason::Rotated,
    InvalidationReason::Revoked,
    InvalidationReason::Destroyed,
];

fn tenant() -> TenantAccount {
    TenantAccount::parse(TENANT).unwrap()
}

fn credential(owner: &str) -> CredentialKey {
    CredentialKey::new(
        CredentialOwnerId::new(owner).unwrap(),
        CredentialId::new(CREDENTIAL_ID).unwrap(),
    )
}

fn appended() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::new(APPENDED_SECONDS, APPENDED_NANOS)
}

fn position(sequence: u64) -> StreamPosition {
    StreamPosition::try_new(sequence).unwrap()
}

fn notice(owner: &str, reason: InvalidationReason) -> InvalidationNotice {
    InvalidationNotice::new(&tenant(), &credential(owner), reason, appended(), position(SEQUENCE)).unwrap()
}

fn decoded(notice: &InvalidationNotice) -> ReleaseInvalidated {
    ReleaseInvalidated::decode_from_slice(notice.payload()).unwrap()
}

/// Whether NATS delivers `subject` to a subscription on `filter`.
fn filter_takes(filter: &str, subject: &str) -> bool {
    let mut subject = subject.split('.');
    for token in filter.split('.') {
        match (token, subject.next()) {
            (">", Some(_)) => return true,
            (_, None) => return false,
            ("*", Some(_)) => {}
            (literal, Some(actual)) if literal == actual => {}
            _ => return false,
        }
    }
    subject.next().is_none()
}

#[test]
fn refuses_an_event_appended_at_the_unix_epoch() {
    let error = InvalidationNotice::new(
        &tenant(),
        &credential("project-1"),
        InvalidationReason::Revoked,
        SystemTime::UNIX_EPOCH,
        position(SEQUENCE),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        InvalidationSourceError::NotAppendedAfterEpoch { sequence: SEQUENCE }
    ));
}

#[test]
fn refuses_an_event_appended_before_the_unix_epoch() {
    let error = InvalidationNotice::new(
        &tenant(),
        &credential("project-1"),
        InvalidationReason::Revoked,
        SystemTime::UNIX_EPOCH - Duration::from_secs(1),
        position(SEQUENCE),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        InvalidationSourceError::NotAppendedAfterEpoch { sequence: SEQUENCE }
    ));
}

#[test]
fn takes_an_event_appended_one_nanosecond_after_the_unix_epoch() {
    let notice = InvalidationNotice::new(
        &tenant(),
        &credential("project-1"),
        InvalidationReason::Revoked,
        SystemTime::UNIX_EPOCH + Duration::from_nanos(1),
        position(SEQUENCE),
    )
    .unwrap();

    let occurred_at = decoded(&notice).occurred_at.into_option().unwrap();
    assert_eq!((occurred_at.seconds, occurred_at.nanos), (0, 1));
}

#[test]
fn the_subject_writes_each_dot_of_the_owner_as_a_tilde() {
    let notice = notice("project.1", InvalidationReason::Revoked);

    assert_eq!(
        notice.subject(),
        "secrets.invalidation.v1.ADVEU3DD4KOFECV66VIHWEZOYX4ZKR3WV27L464SIIPOU2IUI3JCZW22.project~1.\
         0f8fad5b-d9cb-469f-a165-70867728950e"
    );
}

#[test]
fn the_subject_names_the_holding_tenant_after_the_prefix_then_the_owner_then_the_credential() {
    let notice = notice("project-1", InvalidationReason::Revoked);

    assert_eq!(
        notice.subject(),
        "secrets.invalidation.v1.ADVEU3DD4KOFECV66VIHWEZOYX4ZKR3WV27L464SIIPOU2IUI3JCZW22.project-1.\
         0f8fad5b-d9cb-469f-a165-70867728950e"
    );
}

#[test]
fn the_subject_is_one_the_stream_takes_whatever_dots_the_owner_has() {
    for owner in ["project-1", "project.1", "a.b.c", "static-config"] {
        let notice = notice(owner, InvalidationReason::Revoked);

        assert!(
            filter_takes(INVALIDATION_SUBJECTS, notice.subject()),
            "{}",
            notice.subject()
        );
        assert_eq!(notice.subject().split('.').count(), 6, "{}", notice.subject());
    }
}

#[test]
fn the_message_id_is_the_credential_stream_and_the_event_sequence() {
    let notice = notice("project-1", InvalidationReason::Revoked);

    assert_eq!(notice.message_id(), "CREDENTIAL_EVENTS:42");
}

#[test]
fn each_reason_is_sent_as_its_contract_value() {
    let cases = [
        (
            InvalidationReason::Rotated,
            ReleaseInvalidationReason::RELEASE_INVALIDATION_REASON_ROTATED,
        ),
        (
            InvalidationReason::Revoked,
            ReleaseInvalidationReason::RELEASE_INVALIDATION_REASON_REVOKED,
        ),
        (
            InvalidationReason::Destroyed,
            ReleaseInvalidationReason::RELEASE_INVALIDATION_REASON_DESTROYED,
        ),
    ];

    for (reason, wire) in cases {
        assert_eq!(decoded(&notice("project-1", reason)).reason.as_known(), Some(wire));
    }
}

#[test]
fn the_payload_carries_the_credential_the_reason_the_time_and_the_sequence_and_nothing_else() {
    let notice = notice("project.1", InvalidationReason::Revoked);
    let payload = decoded(&notice);

    // Destructured without `..`, so a field added to the contract stops this
    // test compiling until someone decides whether it could hold a value.
    let ReleaseInvalidated {
        credential,
        reason,
        occurred_at,
        source_sequence,
    } = payload.clone();
    let CredentialAddress {
        credential_id,
        owner_id,
    } = credential.into_option().unwrap();
    let occurred_at = occurred_at.into_option().unwrap();

    assert_eq!(credential_id, CREDENTIAL_ID);
    assert_eq!(owner_id, "project.1");
    assert_eq!(
        reason.as_known(),
        Some(ReleaseInvalidationReason::RELEASE_INVALIDATION_REASON_REVOKED)
    );
    assert_eq!(
        (occurred_at.seconds, occurred_at.nanos),
        (
            i64::try_from(APPENDED_SECONDS).unwrap(),
            i32::try_from(APPENDED_NANOS).unwrap()
        )
    );
    assert_eq!(source_sequence, SEQUENCE);
    // Every byte of the payload is one of the fields above.
    assert_eq!(payload.encode_to_vec(), notice.payload());
}

#[test]
fn the_payload_keeps_every_rule_the_contract_validates() {
    for owner in ["project-1", "project.1", "static-config", "team:a_b"] {
        for reason in REASONS {
            let payload = decoded(&notice(owner, reason));

            let credential = payload.credential.as_option().unwrap();
            let canonical = Uuid::try_parse(&credential.credential_id).unwrap().to_string();
            assert_eq!(credential.credential_id, canonical);
            assert!((1..=128).contains(&credential.owner_id.len()));
            assert!(
                credential
                    .owner_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c))
            );
            assert!(!matches!(credential.owner_id.as_str(), "." | ".."));

            assert!(payload.reason.is_known());
            assert_ne!(payload.reason.to_i32(), 0);

            let occurred_at = payload.occurred_at.as_option().unwrap();
            assert!((occurred_at.seconds, occurred_at.nanos) > (0, 0));

            assert!(payload.source_sequence > 0);
        }
    }
}

#[test]
fn the_largest_notice_fits_the_stream_message_size_with_its_headers() {
    let owner = "o.".repeat(64);
    let notice = InvalidationNotice::new(
        &tenant(),
        &credential(&owner),
        InvalidationReason::Destroyed,
        SystemTime::UNIX_EPOCH + Duration::new(u64::try_from(i64::MAX).unwrap(), 999_999_999),
        position(u64::MAX),
    )
    .unwrap();
    // The server measures a message's size as its headers and payload.
    let headers = format!("NATS/1.0\r\nNats-Msg-Id: {}\r\n\r\n", notice.message_id());

    let size = headers.len() + notice.payload().len();

    assert!(
        size <= usize::try_from(INVALIDATION_MAX_MESSAGE_SIZE).unwrap(),
        "{size}"
    );
}
