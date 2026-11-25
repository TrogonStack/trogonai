use std::time::Duration;

use super::ReleaseTimeout;
use crate::{DEFAULT_RELEASE_TIMEOUT, MAX_RELEASE_TIMEOUT, ReleaseTimeoutError};

#[test]
fn refuses_a_zero_timeout() {
    let error = ReleaseTimeout::new(Duration::ZERO).unwrap_err();

    assert_eq!(error, ReleaseTimeoutError::Zero);
    assert_eq!(error.to_string(), "release timeout must be longer than zero");
}

#[test]
fn refuses_a_timeout_past_the_maximum_instead_of_clamping_it() {
    let requested = MAX_RELEASE_TIMEOUT + Duration::from_millis(1);

    let error = ReleaseTimeout::new(requested).unwrap_err();

    assert_eq!(
        error,
        ReleaseTimeoutError::TooLong {
            requested,
            max: MAX_RELEASE_TIMEOUT,
        }
    );
    assert_eq!(
        error.to_string(),
        "release timeout of 5.001s is longer than the 5s maximum"
    );
}

#[test]
fn accepts_the_maximum() {
    let timeout = ReleaseTimeout::new(MAX_RELEASE_TIMEOUT).unwrap();

    assert_eq!(timeout.get(), MAX_RELEASE_TIMEOUT);
}

#[test]
fn defaults_to_two_seconds() {
    assert_eq!(ReleaseTimeout::default().get(), DEFAULT_RELEASE_TIMEOUT);
    assert_eq!(DEFAULT_RELEASE_TIMEOUT, Duration::from_secs(2));
}
