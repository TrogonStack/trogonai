use std::error::Error as _;

use trogon_nats::mocks::MockError;

use super::*;

#[test]
fn an_unreadable_delivery_keeps_why_as_its_source() {
    let error = DeliveryInfoError(Box::new(MockError("fake-unparsable-reply".to_owned())));

    assert_eq!(
        error.to_string(),
        "the delivery does not say where the credential stream stored the message"
    );
    assert!(error.source().is_some_and(|source| source.is::<MockError>()));
}
