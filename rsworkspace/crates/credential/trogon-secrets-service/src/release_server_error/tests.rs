use std::error::Error as _;

use trogon_nats::mocks::MockError;

use super::*;

#[test]
fn a_refused_join_keeps_the_bus_error_as_its_source() {
    let error = ReleaseServerError::Join(MockError("fake-refusal".to_owned()));
    assert_eq!(error.to_string(), "release endpoint could not join its queue group");
    assert!(error.source().is_some_and(|source| source.is::<MockError>()));
}

#[test]
fn an_ended_subscription_says_it_ended_before_shutdown() {
    let error = ReleaseServerError::<MockError>::SubscriptionEnded;
    assert_eq!(error.to_string(), "release subscription ended before shutdown");
    assert!(error.source().is_none());
}
