#![allow(clippy::unwrap_used)]

use chrono::{DateTime, Utc};
use trogon_decider::testing::TestCase;
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
fn enrollment_rejects_chrono_leap_seconds_before_protobuf_conversion() {
    for nanos in [1_000_000_000, 1_500_000_000, 1_999_999_999] {
        let leap_second = DateTime::from_timestamp(1_483_228_799, nanos).unwrap();
        TestCase::<Enroll>::new()
            .given_no_history()
            .when(command(leap_second))
            .then_error(EnrollError::LeaseDeadlineOverflow);
    }
}
