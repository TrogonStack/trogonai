use super::*;

#[test]
fn a_bucket_name_is_what_nats_accepts_as_one() {
    assert_eq!(ClaimBucket::new("trogon-claims").expect("valid").as_str(), "trogon-claims");
    assert_eq!(ClaimBucket::new("claims_2").expect("valid").as_str(), "claims_2");
}

/// A `.` would name a subject, and a `/` a path inside a bucket; both are names
/// JetStream rejects, and rejecting them here means the operator hears about it
/// at startup instead of on the first oversized message.
#[test]
fn a_name_nats_would_refuse_is_refused_here() {
    assert_eq!(ClaimBucket::new("").unwrap_err(), ClaimBucketError::Empty);
    assert_eq!(
        ClaimBucket::new("trogon.claims").unwrap_err(),
        ClaimBucketError::InvalidCharacter('.')
    );
    assert_eq!(
        ClaimBucket::new("trogon claims").unwrap_err(),
        ClaimBucketError::InvalidCharacter(' ')
    );
    assert_eq!(
        ClaimBucket::new("claims/one").unwrap_err(),
        ClaimBucketError::InvalidCharacter('/')
    );
}

/// [`ClaimBucket::default`] skips validation because the constant cannot fail.
/// This is the test that makes that true rather than assumed.
#[test]
fn the_default_bucket_is_a_valid_name() {
    assert_eq!(
        ClaimBucket::new(DEFAULT_CLAIM_BUCKET).expect("the default bucket must be a valid name"),
        ClaimBucket::default()
    );
    assert_eq!(ClaimBucket::default().to_string(), DEFAULT_CLAIM_BUCKET);
}
