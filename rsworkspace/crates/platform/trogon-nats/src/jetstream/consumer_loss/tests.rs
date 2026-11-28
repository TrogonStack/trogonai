use super::*;

#[test]
fn lost_consumer_errors_require_a_new_subscription() {
    for kind in [
        MessagesErrorKind::ConsumerDeleted,
        MessagesErrorKind::NoResponders,
        MessagesErrorKind::Pull,
    ] {
        assert!(MessagesError::new(kind).consumer_lost(), "{kind:?}");
    }
}

#[test]
fn passing_errors_keep_the_subscription() {
    for kind in [MessagesErrorKind::MissingHeartbeat, MessagesErrorKind::Other] {
        assert!(!MessagesError::new(kind).consumer_lost(), "{kind:?}");
    }
}
