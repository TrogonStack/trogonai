use std::error::Error as _;

use nkeys::error::ErrorKind;
use nkeys::{KeyPair, XKey};

use super::ServiceXkey;
use crate::ServiceXkeyError;

#[test]
fn refuses_a_value_that_is_not_a_key() {
    let error = ServiceXkey::from_public_key("fake-service-xkey").unwrap_err();

    assert!(matches!(&error, ServiceXkeyError::NotACurvePublicKey(source) if source.kind() == ErrorKind::CodecFailure));
    assert!(error.source().is_some());
    assert_eq!(error.to_string(), "service xkey is not a curve public key");
}

#[test]
fn refuses_a_public_key_that_is_not_a_curve_key() {
    let error = ServiceXkey::from_public_key(&KeyPair::new_account().public_key()).unwrap_err();

    assert!(matches!(error, ServiceXkeyError::NotACurvePublicKey(source) if source.kind() == ErrorKind::InvalidPrefix));
}

#[test]
fn refuses_a_curve_seed() {
    let seed = XKey::new().seed().unwrap();

    let error = ServiceXkey::from_public_key(&seed).unwrap_err();

    assert!(matches!(error, ServiceXkeyError::NotACurvePublicKey(source) if source.kind() == ErrorKind::InvalidPrefix));
}

#[test]
fn shows_only_the_public_key() {
    let key = XKey::new().public_key();

    let pinned = ServiceXkey::from_public_key(&key).unwrap();

    assert_eq!(pinned.public_key(), key);
    assert_eq!(format!("{pinned:?}"), format!("ServiceXkey({key:?})"));
}
