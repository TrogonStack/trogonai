use std::time::Duration;

use super::*;
use crate::fixtures::now;

#[test]
fn both_ends_are_inside() {
    let second = Duration::from_secs(1);
    let bounds = TimeBounds::new(now(), now() + second);

    assert_eq!(bounds.earliest(), now());
    assert_eq!(bounds.latest(), now() + second);
    assert!(bounds.contains(now()));
    assert!(bounds.contains(now() + second));
    assert!(!bounds.contains(now() - second));
    assert!(!bounds.contains(now() + second * 2));
}
