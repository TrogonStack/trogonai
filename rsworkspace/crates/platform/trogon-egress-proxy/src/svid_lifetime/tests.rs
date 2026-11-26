use std::time::{Duration, SystemTime};

use rcgen::{CertificateParams, KeyPair};

use super::*;

fn leaf(not_before: SystemTime, not_after: SystemTime) -> Vec<u8> {
    let mut params = CertificateParams::default();
    params.not_before = not_before.into();
    params.not_after = not_after.into();
    let key = KeyPair::generate().expect("key");
    params.self_signed(&key).expect("leaf").der().to_vec()
}

#[test]
fn accepts_one_hour_and_shorter() {
    let now = SystemTime::now();
    for seconds in [1, 300, 3599, 3600] {
        let der = leaf(now, now + Duration::from_secs(seconds));
        assert_eq!(verify_svid_lifetime(&der), Ok(()), "{seconds}s");
    }
}

#[test]
fn refuses_longer_than_one_hour_and_names_the_ceiling() {
    let now = SystemTime::now();
    let der = leaf(now, now + Duration::from_secs(3601));
    assert_eq!(
        verify_svid_lifetime(&der),
        Err(SvidLifetimeError::TooLong {
            lifetime: Duration::from_secs(3601),
            ceiling: Duration::from_secs(3600),
        })
    );
}

#[test]
fn refuses_a_default_multi_decade_certificate() {
    let der = leaf(
        SystemTime::UNIX_EPOCH + Duration::from_secs(1),
        SystemTime::UNIX_EPOCH + Duration::from_secs(4_000_000_000),
    );
    assert!(matches!(
        verify_svid_lifetime(&der),
        Err(SvidLifetimeError::TooLong { .. })
    ));
}

#[test]
fn refuses_empty_and_inverted_validity_and_garbage() {
    let now = SystemTime::now();
    let empty = leaf(now, now);
    let inverted = leaf(now, now - Duration::from_secs(60));
    for der in [empty, inverted, vec![0u8; 8]] {
        assert_eq!(
            verify_svid_lifetime(&der),
            Err(SvidLifetimeError::Unreadable(MaterialError::Profile))
        );
    }
}
