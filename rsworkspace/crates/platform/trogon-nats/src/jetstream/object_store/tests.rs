use std::time::Duration;

use super::widened_max_age;

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
