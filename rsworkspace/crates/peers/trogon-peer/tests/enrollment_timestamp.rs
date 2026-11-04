#![allow(clippy::unwrap_used)]

use chrono::{DateTime, Utc};
use trogon_decider::Decider;
use trogon_peer::{
    CallerId, CollaborationRef, ConnectorId, Enroll, EnrollError, EnrollmentIdentity, InstallationId, NativeSessionId,
    PeerDisplayName, TenantId, TrustedPeerOwner,
};

fn command(now: DateTime<Utc>) -> Enroll {
    Enroll::new(
        EnrollmentIdentity::new(
            TrustedPeerOwner::new(TenantId::new("tenant").unwrap(), CallerId::new("caller").unwrap()),
            InstallationId::new("installation").unwrap(),
            NativeSessionId::new("session").unwrap(),
        ),
        ConnectorId::new("connector").unwrap(),
        CollaborationRef::new("collaboration").unwrap(),
        PeerDisplayName::new("Peer").unwrap(),
        now,
    )
}

#[test]
fn enrollment_accepts_protobuf_nanosecond_boundaries() {
    for nanos in [0, 999_999_999] {
        let now = DateTime::from_timestamp(1_483_228_799, nanos).unwrap();
        assert!(Enroll::decide(&Enroll::initial_state(), &command(now)).is_ok());
    }
}

#[test]
fn enrollment_rejects_chrono_leap_seconds_before_protobuf_conversion() {
    for nanos in [1_000_000_000, 1_500_000_000, 1_999_999_999] {
        let leap_second = DateTime::from_timestamp(1_483_228_799, nanos).unwrap();
        assert!(matches!(
            Enroll::decide(&Enroll::initial_state(), &command(leap_second)),
            Err(EnrollError::LeaseDeadlineOverflow)
        ));
    }
}
