use super::*;

#[test]
fn max_outstanding_messages_rejects_zero() {
    assert_eq!(MaxOutstandingMessages::new(0), Err(MaxOutstandingMessagesError));
}

#[test]
fn max_outstanding_messages_keeps_value() {
    assert_eq!(MaxOutstandingMessages::new(250).unwrap().get(), 250);
}
