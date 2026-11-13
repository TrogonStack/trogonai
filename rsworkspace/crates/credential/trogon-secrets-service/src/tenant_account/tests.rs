use nkeys::KeyPair;

use super::*;

#[test]
fn refuses_a_value_that_is_not_a_public_key() {
    assert_eq!(
        TenantAccount::parse("not-a-key"),
        Err(TenantAccountError::NotAPublicKey)
    );
}

#[test]
fn refuses_a_public_key_that_is_not_an_account() {
    let user = KeyPair::new_user().public_key();

    assert_eq!(TenantAccount::parse(&user), Err(TenantAccountError::NotAnAccount));
}

#[test]
fn accepts_an_account_public_key() {
    let account = KeyPair::new_account().public_key();

    let tenant = TenantAccount::parse(&account).unwrap();

    assert_eq!(tenant.as_str(), account);
    assert_eq!(tenant.to_string(), account);
}

#[test]
fn errors_say_what_was_wrong() {
    assert_eq!(
        TenantAccountError::NotAPublicKey.to_string(),
        "tenant account is not an NKey public key"
    );
    assert_eq!(
        TenantAccountError::NotAnAccount.to_string(),
        "tenant account is an NKey public key that names something other than an account"
    );
}
