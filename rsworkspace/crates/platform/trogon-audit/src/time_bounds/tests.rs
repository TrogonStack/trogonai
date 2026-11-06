use std::time::Duration;

use super::*;
use crate::fixtures::now;

#[test]
fn both_ends_are_inside() {
    let second = Duration::from_secs(1);
    let bounds = TimeBounds::new(now(), now() + second).unwrap();

    assert_eq!(bounds.earliest(), now());
    assert_eq!(bounds.latest(), now() + second);
    assert!(bounds.contains(now()));
    assert!(bounds.contains(now() + second));
    assert!(!bounds.contains(now() - second));
    assert!(!bounds.contains(now() + second * 2));
}

#[test]
fn construction_never_admits_an_inverted_window() {
    assert_eq!(
        TimeBounds::new(now() + Duration::from_secs(1), now()),
        Err(TimeBoundsError)
    );
}

#[test]
fn equal_bounds_accept_exactly_that_time() {
    let bounds = TimeBounds::new(now(), now()).unwrap();

    assert!(bounds.contains(now()));
    assert!(!bounds.contains(now() - Duration::from_nanos(1)));
    assert!(!bounds.contains(now() + Duration::from_nanos(1)));
}

#[test]
fn the_error_names_the_invalid_window() {
    assert_eq!(
        TimeBoundsError.to_string(),
        "checkpoint time bounds must start no later than they end"
    );
}
