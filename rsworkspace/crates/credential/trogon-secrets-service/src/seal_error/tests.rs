use nkeys::XKey;

use super::*;

#[test]
fn says_why_the_value_could_not_be_sealed() {
    let error = SealError::new("no private key");

    assert_eq!(error.message(), "no private key");
    assert_eq!(error.to_string(), "value could not be sealed: no private key");
}

#[test]
fn a_key_without_a_private_half_cannot_seal() {
    let public_only = XKey::from_public_key(&XKey::new().public_key()).unwrap();

    let error = SealError::from(public_only.seal(b"value", &XKey::new()).unwrap_err());

    assert!(!error.message().is_empty());
}

#[test]
fn an_empty_sealed_value_cannot_seal() {
    assert_eq!(
        SealError::from(SealedValueError::Empty).message(),
        "sealed value must not be empty"
    );
}
