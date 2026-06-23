use super::*;

fn system_time_error() -> std::time::SystemTimeError {
    let now = std::time::SystemTime::now();
    now.duration_since(now + std::time::Duration::from_secs(60))
        .unwrap_err()
}

#[test]
fn decode_variant_preserves_detail() {
    let JwtError::Decode(detail) = JwtError::Decode("oops".into()) else {
        panic!("expected Decode variant");
    };
    assert_eq!(detail, "oops");
}

#[test]
fn every_variant_is_distinct() {
    assert!(matches!(JwtError::Decode("oops".into()), JwtError::Decode(_)));
    assert!(matches!(
        JwtError::SystemTime(system_time_error()),
        JwtError::SystemTime(_)
    ));
    assert!(matches!(JwtError::InvalidCallerId, JwtError::InvalidCallerId));
    assert!(matches!(
        JwtError::InvalidExternalSubject,
        JwtError::InvalidExternalSubject
    ));
    assert!(matches!(JwtError::IssuedAtOutOfRange, JwtError::IssuedAtOutOfRange));
}

#[test]
fn system_time_variant_exposes_source() {
    let err = JwtError::SystemTime(system_time_error());
    assert!(std::error::Error::source(&err).is_some());
    assert!(std::error::Error::source(&JwtError::InvalidCallerId).is_none());
}
