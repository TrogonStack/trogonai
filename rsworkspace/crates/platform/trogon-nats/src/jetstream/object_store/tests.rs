use std::time::Duration;

use super::{ClaimBucket, ClaimBucketBinding, widened_max_age};

const HOUR: Duration = Duration::from_secs(3600);
const NO_EXPIRY: Duration = Duration::ZERO;

#[test]
fn grows_to_a_longer_finite_retention() {
    assert_eq!(widened_max_age(HOUR, 2 * HOUR), Some(2 * HOUR));
}

#[test]
fn never_shrinks_to_a_shorter_finite_retention() {
    assert_eq!(widened_max_age(2 * HOUR, HOUR), None);
}

#[test]
fn equal_retention_is_a_no_op() {
    assert_eq!(widened_max_age(HOUR, HOUR), None);
}

#[test]
fn grows_from_finite_to_no_expiry() {
    assert_eq!(widened_max_age(HOUR, NO_EXPIRY), Some(NO_EXPIRY));
}

#[test]
fn never_shrinks_from_no_expiry_to_finite() {
    assert_eq!(widened_max_age(NO_EXPIRY, HOUR), None);
}

#[test]
fn no_expiry_to_no_expiry_is_a_no_op() {
    assert_eq!(widened_max_age(NO_EXPIRY, NO_EXPIRY), None);
}

/// A consumer validates the claim's `Nats-Claim-Bucket` header against
/// `bucket()` while reading through the handle, and a publisher that only writes
/// that header takes `into_store()` and drops the name. Both halves have to come
/// back out as the halves that went in, or the validation passes on one bucket
/// while the handle reads another.
#[test]
fn a_binding_hands_each_half_back_as_the_half_it_was_given() {
    let bucket = ClaimBucket::new("claims").expect("valid bucket name");
    let binding = ClaimBucketBinding::for_test("a-store-handle", bucket.clone());

    assert_eq!(binding.bucket(), &bucket);
    assert_eq!(binding.into_store(), "a-store-handle");
}
