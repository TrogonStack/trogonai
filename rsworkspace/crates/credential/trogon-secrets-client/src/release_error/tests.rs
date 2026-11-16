use std::error::Error;
use std::time::Duration;

use buffa::{EnumValue, MessageField};
use buffa_types::DurationError;
use buffa_types::google::protobuf::Duration as WireDuration;
use grpc_nats_micro::client::RequestError;
use trogon_nats::mocks::MockError;
use trogonai_proto::secrets::exchange::v1alpha1::{ResolveFailure, ResolveFailureCategory as Category};

use super::ReleaseError;
use crate::InvalidReleaseResponseError;

fn failure(category: impl Into<EnumValue<Category>>, retry_after: Option<WireDuration>) -> ResolveFailure {
    ResolveFailure {
        category: category.into(),
        retry_after: retry_after.map_or_else(MessageField::none, MessageField::some),
    }
}

fn seconds(seconds: i64) -> WireDuration {
    WireDuration {
        seconds,
        ..WireDuration::default()
    }
}

#[test]
fn refuses_a_category_it_does_not_know() {
    let refused = ReleaseError::reported(failure(99, None)).unwrap_err();

    assert!(matches!(refused, InvalidReleaseResponseError::UnknownCategory(99)));
}

#[test]
fn refuses_the_categories_a_release_never_answers() {
    for category in [
        Category::RESOLVE_FAILURE_CATEGORY_UNSPECIFIED,
        Category::RESOLVE_FAILURE_CATEGORY_RECONNECT_REQUIRED,
        Category::RESOLVE_FAILURE_CATEGORY_REFRESH_PENDING,
    ] {
        let refused = ReleaseError::reported(failure(category, None)).unwrap_err();

        assert!(
            matches!(refused, InvalidReleaseResponseError::UnexpectedCategory(code) if code == category as i32),
            "{category:?}"
        );
    }
}

#[test]
fn refuses_a_throttle_without_retry_after() {
    let refused = ReleaseError::reported(failure(Category::RESOLVE_FAILURE_CATEGORY_THROTTLED, None)).unwrap_err();

    assert!(matches!(
        refused,
        InvalidReleaseResponseError::ThrottledWithoutRetryAfter
    ));
}

#[test]
fn refuses_a_negative_retry_after() {
    let refused =
        ReleaseError::reported(failure(Category::RESOLVE_FAILURE_CATEGORY_THROTTLED, Some(seconds(-1)))).unwrap_err();

    assert!(matches!(
        refused,
        InvalidReleaseResponseError::RetryAfter(DurationError::NegativeDuration)
    ));
}

#[test]
fn refuses_a_retry_after_on_any_category_but_throttled() {
    for category in [
        Category::RESOLVE_FAILURE_CATEGORY_DENIED,
        Category::RESOLVE_FAILURE_CATEGORY_KEY_DENIED,
        Category::RESOLVE_FAILURE_CATEGORY_UNAVAILABLE,
        Category::RESOLVE_FAILURE_CATEGORY_INTEGRITY_FAILURE,
        Category::RESOLVE_FAILURE_CATEGORY_INTERNAL,
        Category::RESOLVE_FAILURE_CATEGORY_REFRESH_PENDING,
    ] {
        let refused = ReleaseError::reported(failure(category, Some(seconds(1)))).unwrap_err();

        assert!(
            matches!(refused, InvalidReleaseResponseError::RetryAfterOutsideThrottled),
            "{category:?}"
        );
    }
}

#[test]
fn reads_each_refusal_a_release_may_answer() {
    let read = |category| ReleaseError::reported(failure(category, None)).unwrap();

    assert!(matches!(
        read(Category::RESOLVE_FAILURE_CATEGORY_DENIED),
        ReleaseError::Denied
    ));
    assert!(matches!(
        read(Category::RESOLVE_FAILURE_CATEGORY_KEY_DENIED),
        ReleaseError::KeyDenied
    ));
    assert!(matches!(
        read(Category::RESOLVE_FAILURE_CATEGORY_UNAVAILABLE),
        ReleaseError::Unavailable
    ));
    assert!(matches!(
        read(Category::RESOLVE_FAILURE_CATEGORY_INTEGRITY_FAILURE),
        ReleaseError::IntegrityFailure
    ));
    assert!(matches!(
        read(Category::RESOLVE_FAILURE_CATEGORY_INTERNAL),
        ReleaseError::Internal
    ));
}

#[test]
fn reads_a_throttle_with_its_retry_after() {
    let read = ReleaseError::reported(failure(Category::RESOLVE_FAILURE_CATEGORY_THROTTLED, Some(seconds(3)))).unwrap();

    assert!(matches!(read, ReleaseError::Throttled { retry_after } if retry_after == Duration::from_secs(3)));
}

#[test]
fn reads_a_throttle_that_allows_retrying_at_once() {
    let read = ReleaseError::reported(failure(Category::RESOLVE_FAILURE_CATEGORY_THROTTLED, Some(seconds(0)))).unwrap();

    assert!(matches!(read, ReleaseError::Throttled { retry_after } if retry_after == Duration::ZERO));
}

#[test]
fn reads_a_timeout_as_a_timeout() {
    let error = ReleaseError::from_request(RequestError::<MockError>::Timeout {
        subject: "fake.subject".to_owned(),
    });

    assert!(matches!(error, ReleaseError::Timeout));
}

#[test]
fn reads_a_transport_failure_as_a_request_failure() {
    let error = ReleaseError::from_request(RequestError::Transport {
        subject: "fake.subject".to_owned(),
        error: MockError("fake-unreachable".to_owned()),
    });

    assert!(matches!(error, ReleaseError::Request(_)));
}

#[test]
fn keeps_the_cause_of_a_request_failure_in_every_copy() {
    let error = ReleaseError::from_request(RequestError::Transport {
        subject: "fake.subject".to_owned(),
        error: MockError("fake-unreachable".to_owned()),
    });

    let copy = error.clone();

    assert!(copy.source().is_some());
    assert_eq!(
        copy.source().map(ToString::to_string),
        error.source().map(ToString::to_string)
    );
}

#[test]
fn says_nothing_about_why_a_release_was_denied() {
    assert_eq!(ReleaseError::Denied.to_string(), "release denied");
}
