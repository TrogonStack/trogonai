use std::error::Error as _;

use nkeys::KeyPair;
use nkeys::error::ErrorKind;

use super::TenantAccount;
use crate::TenantAccountError;

#[test]
fn refuses_a_value_that_is_not_a_public_key() {
    let error = TenantAccount::parse("fake-tenant").unwrap_err();

    assert!(
        matches!(&error, TenantAccountError::NotAPublicKey(source) if source.kind() == ErrorKind::InvalidKeyLength)
    );
    assert!(error.source().is_some());
    assert_eq!(error.to_string(), "tenant account is not an NKey public key");
}

#[test]
fn refuses_a_public_key_that_names_a_user() {
    let error = TenantAccount::parse(&KeyPair::new_user().public_key()).unwrap_err();

    assert!(matches!(error, TenantAccountError::NotAnAccount));
    assert_eq!(
        error.to_string(),
        "tenant account is an NKey public key that names something other than an account"
    );
}

#[test]
fn keeps_an_account_public_key_as_given() {
    let key = KeyPair::new_account().public_key();

    let tenant = TenantAccount::parse(&key).unwrap();

    assert_eq!(tenant.as_str(), key);
    assert_eq!(tenant.to_string(), key);
}
