use super::*;

#[test]
fn a_bucket_name_is_what_nats_accepts_as_one() {
    assert_eq!(
        ClaimBucket::new("trogon-claims").expect("valid").as_str(),
        "trogon-claims"
    );
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
    assert_eq!(
        ClaimBucket::new("claims-ñ").unwrap_err(),
        ClaimBucketError::InvalidCharacter('ñ')
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

/// A header is input, so what it holds may not be a bucket name at all. Both
/// answers matter: the name to compare against the bucket this consumer opened,
/// and the text an operator has to read when there is nothing to compare.
#[test]
fn a_header_parses_to_a_bucket_name_or_says_why_it_cannot() {
    let named = ClaimBucketHeader::new("trogon-claims");
    assert_eq!(named.parse().expect("valid"), ClaimBucket::default());
    assert_eq!(named.as_str(), "trogon-claims");
    assert_eq!(named.to_string(), "trogon-claims");

    let unnamable = ClaimBucketHeader::new("trogon.claims");
    assert_eq!(unnamable.parse().unwrap_err(), ClaimBucketError::InvalidCharacter('.'));
    assert_eq!(unnamable.to_string(), "trogon.claims");
}
