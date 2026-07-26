use super::*;

#[test]
fn no_expiry_is_zero() {
    assert_eq!(Duration::from(StreamMaxAge::NoExpiry), Duration::ZERO);
}

#[test]
fn expire_after() {
    let age = StreamMaxAge::from_secs(3600).unwrap();
    assert_eq!(Duration::from(age), Duration::from_secs(3600));
}

#[test]
fn zero_rejected() {
    assert!(matches!(StreamMaxAge::from_secs(0), Err(ZeroDurationError)));
}
