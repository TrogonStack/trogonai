use std::time::{Duration, SystemTime};

use buffa::MessageField;
use buffa_types::TimestampError;
use buffa_types::google::protobuf::Timestamp;
use nkeys::XKey;
use nkeys::error::ErrorKind;

use super::ReleasedValue;
use crate::fixtures::{BUNDLE_GENERATION, VALUE, now, pinned, sealed_access};
use crate::{InvalidReleaseResponseError, RELEASE_HOLD_BOUND, ReleaseError, UnsealError};

fn in_a_minute() -> SystemTime {
    now() + Duration::from_secs(60)
}

fn open_sealed(plain: &[u8], valid_until: SystemTime) -> Result<ReleasedValue, ReleaseError> {
    let service = XKey::new();
    let requester = XKey::new();
    let access = sealed_access(&service, &requester, plain, valid_until);
    ReleasedValue::open(access, &requester, &pinned(&service), now())
}

#[test]
fn refuses_an_empty_sealed_value() {
    let service = XKey::new();
    let requester = XKey::new();
    let mut access = sealed_access(&service, &requester, VALUE.as_bytes(), in_a_minute());
    access.xkey_sealed_value.clear();

    let refused = ReleasedValue::open(access, &requester, &pinned(&service), now()).unwrap_err();

    assert!(matches!(
        refused,
        ReleaseError::InvalidResponse(InvalidReleaseResponseError::EmptySealedValue)
    ));
}

#[test]
fn refuses_an_access_without_valid_until() {
    let service = XKey::new();
    let requester = XKey::new();
    let mut access = sealed_access(&service, &requester, VALUE.as_bytes(), in_a_minute());
    access.valid_until = MessageField::none();

    let refused = ReleasedValue::open(access, &requester, &pinned(&service), now()).unwrap_err();

    assert!(matches!(
        refused,
        ReleaseError::InvalidResponse(InvalidReleaseResponseError::MissingValidUntil)
    ));
}

#[test]
fn refuses_a_valid_until_at_or_before_the_epoch() {
    let service = XKey::new();
    let requester = XKey::new();
    let mut access = sealed_access(&service, &requester, VALUE.as_bytes(), in_a_minute());
    access.valid_until = MessageField::some(Timestamp {
        seconds: 0,
        nanos: 5,
        ..Timestamp::default()
    });

    let refused = ReleasedValue::open(access, &requester, &pinned(&service), now()).unwrap_err();

    assert!(matches!(
        refused,
        ReleaseError::InvalidResponse(InvalidReleaseResponseError::ValidUntilNotPositive)
    ));
}

#[test]
fn refuses_a_valid_until_with_out_of_range_nanos() {
    let service = XKey::new();
    let requester = XKey::new();
    let mut access = sealed_access(&service, &requester, VALUE.as_bytes(), in_a_minute());
    access.valid_until = MessageField::some(Timestamp {
        seconds: 1_700_000_060,
        nanos: 1_000_000_000,
        ..Timestamp::default()
    });

    let refused = ReleasedValue::open(access, &requester, &pinned(&service), now()).unwrap_err();

    assert!(matches!(
        refused,
        ReleaseError::InvalidResponse(InvalidReleaseResponseError::ValidUntil(TimestampError::InvalidNanos))
    ));
}

#[test]
fn refuses_a_value_that_expired_before_it_arrived() {
    let refused = open_sealed(VALUE.as_bytes(), now() - Duration::from_secs(1)).unwrap_err();

    assert!(matches!(refused, ReleaseError::Expired));
}

#[test]
fn refuses_a_value_that_expires_the_moment_it_arrives() {
    let refused = open_sealed(VALUE.as_bytes(), now()).unwrap_err();

    assert!(matches!(refused, ReleaseError::Expired));
}

#[test]
fn refuses_a_value_sealed_by_a_key_other_than_the_pinned_one() {
    let impostor = XKey::new();
    let requester = XKey::new();
    let access = sealed_access(&impostor, &requester, VALUE.as_bytes(), in_a_minute());

    let refused = ReleasedValue::open(access, &requester, &pinned(&XKey::new()), now()).unwrap_err();

    assert!(
        matches!(refused, ReleaseError::Unseal(UnsealError::NotOpened(source)) if source.kind() == ErrorKind::InvalidPayload)
    );
}

#[test]
fn refuses_a_value_sealed_to_another_requester() {
    let service = XKey::new();
    let access = sealed_access(&service, &XKey::new(), VALUE.as_bytes(), in_a_minute());

    let refused = ReleasedValue::open(access, &XKey::new(), &pinned(&service), now()).unwrap_err();

    assert!(
        matches!(refused, ReleaseError::Unseal(UnsealError::NotOpened(source)) if source.kind() == ErrorKind::InvalidPayload)
    );
}

#[test]
fn refuses_a_value_that_is_not_utf8() {
    let refused = open_sealed(&[0xff, 0xfe], in_a_minute()).unwrap_err();

    assert!(matches!(refused, ReleaseError::Unseal(UnsealError::NotUtf8)));
}

#[test]
fn refuses_an_empty_value() {
    let refused = open_sealed(b"", in_a_minute()).unwrap_err();

    assert!(matches!(refused, ReleaseError::Unseal(UnsealError::Empty)));
}

#[test]
fn opens_the_value_sealed_by_the_pinned_key() {
    let released = open_sealed(VALUE.as_bytes(), in_a_minute()).unwrap();

    assert_eq!(released.value().as_str(), VALUE);
    assert_eq!(released.valid_until(), in_a_minute());
    assert_eq!(released.bundle_generation(), BUNDLE_GENERATION);
}

#[test]
fn holds_a_value_until_its_valid_until_when_that_comes_first() {
    let released = open_sealed(VALUE.as_bytes(), in_a_minute()).unwrap();

    assert_eq!(released.hold_until(), in_a_minute());
    assert!(released.is_held_at(in_a_minute() - Duration::from_secs(1)));
    assert!(!released.is_held_at(in_a_minute()));
}

#[test]
fn holds_a_value_no_longer_than_the_hold_bound() {
    let valid_until = now() + Duration::from_secs(3_600);

    let released = open_sealed(VALUE.as_bytes(), valid_until).unwrap();

    assert_eq!(released.valid_until(), valid_until);
    assert_eq!(released.hold_until(), now() + RELEASE_HOLD_BOUND);
    assert!(!released.is_held_at(now() + RELEASE_HOLD_BOUND));
}
