use std::error::Error as _;

use trogon_nats::mocks::MockError;

use super::*;

type Error = InvalidationPublisherError<MockError, MockError>;

#[test]
fn a_consumer_that_could_not_be_created_names_the_durable_and_keeps_the_cause() {
    let error = Error::Consumer(MockError("fake-refusal".to_owned()));

    assert_eq!(
        error.to_string(),
        "durable consumer 'secrets-invalidation-publisher' could not be created on the credential stream"
    );
    assert!(error.source().is_some_and(|source| source.is::<MockError>()));
}

#[test]
fn messages_that_could_not_start_name_the_durable_and_keep_the_cause() {
    let error = Error::Messages(MockError("fake-refusal".to_owned()));

    assert_eq!(
        error.to_string(),
        "durable consumer 'secrets-invalidation-publisher' could not start delivering credential events"
    );
    assert!(error.source().is_some_and(|source| source.is::<MockError>()));
}

#[test]
fn messages_that_ended_say_they_ended_before_shutdown() {
    let error = Error::MessagesEnded;

    assert_eq!(
        error.to_string(),
        "durable consumer 'secrets-invalidation-publisher' stopped delivering credential events before shutdown"
    );
    assert!(error.source().is_none());
}
