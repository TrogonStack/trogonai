use super::*;

#[test]
fn each_message_names_the_rule_and_the_stream() {
    let cases = [
        (
            InvalidationStreamMismatchError::Storage,
            "stream 'SECRETS_INVALIDATION' must use file storage",
        ),
        (
            InvalidationStreamMismatchError::Retention,
            "stream 'SECRETS_INVALIDATION' must use limits retention",
        ),
        (
            InvalidationStreamMismatchError::DenyDelete,
            "stream 'SECRETS_INVALIDATION' must deny deleting a notice",
        ),
        (
            InvalidationStreamMismatchError::DenyPurge,
            "stream 'SECRETS_INVALIDATION' must deny purging notices",
        ),
        (
            InvalidationStreamMismatchError::Rollup,
            "stream 'SECRETS_INVALIDATION' must not let a publish roll up and remove stored notices",
        ),
        (
            InvalidationStreamMismatchError::NoAck,
            "stream 'SECRETS_INVALIDATION' must acknowledge publishes",
        ),
        (
            InvalidationStreamMismatchError::Sealed,
            "stream 'SECRETS_INVALIDATION' must not be sealed, since a sealed stream stores no new notice",
        ),
        (
            InvalidationStreamMismatchError::AsyncPersist,
            "stream 'SECRETS_INVALIDATION' must store a notice before acknowledging it",
        ),
        (
            InvalidationStreamMismatchError::MaxAge,
            "stream 'SECRETS_INVALIDATION' must keep notices for the 330 second replay window or longer",
        ),
        (
            InvalidationStreamMismatchError::MessageTtl,
            "stream 'SECRETS_INVALIDATION' must not let a notice set its own expiry",
        ),
        (
            InvalidationStreamMismatchError::MessageCounter,
            "stream 'SECRETS_INVALIDATION' must not be a counter stream, since a counter stream refuses every notice",
        ),
        (
            InvalidationStreamMismatchError::MaxMessagesPerSubject,
            "stream 'SECRETS_INVALIDATION' must not limit the notices kept per subject",
        ),
        (
            InvalidationStreamMismatchError::MaxMessageSize,
            "stream 'SECRETS_INVALIDATION' must take messages of 1024 bytes or more",
        ),
        (
            InvalidationStreamMismatchError::DuplicateWindow,
            "stream 'SECRETS_INVALIDATION' must keep a duplicate window of two minutes or longer",
        ),
        (
            InvalidationStreamMismatchError::SubjectTransform,
            "stream 'SECRETS_INVALIDATION' must not transform the subjects notices are stored on",
        ),
        (
            InvalidationStreamMismatchError::Replicas,
            "stream 'SECRETS_INVALIDATION' must keep at least the replicas the deployment asked for",
        ),
        (
            InvalidationStreamMismatchError::MissingSubject,
            "stream 'SECRETS_INVALIDATION' is missing the subject notices are published on: 'secrets.invalidation.v1.*.>'",
        ),
    ];
    for (error, message) in cases {
        assert_eq!(error.to_string(), message);
    }
}
