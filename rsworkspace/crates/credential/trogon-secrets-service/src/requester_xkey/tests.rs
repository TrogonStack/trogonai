use nkeys::{KeyPair, XKey};

use super::*;

#[test]
fn refuses_a_value_that_is_not_a_key() {
    assert_eq!(
        RequesterXkey::parse("not-a-key").unwrap_err(),
        RequesterXkeyError::NotACurveKey
    );
}

#[test]
fn refuses_a_public_key_that_is_not_a_curve_key() {
    let account = KeyPair::new_account().public_key();

    assert_eq!(
        RequesterXkey::parse(&account).unwrap_err(),
        RequesterXkeyError::NotACurveKey
    );
}

#[test]
fn refuses_a_curve_seed() {
    let seed = XKey::new().seed().unwrap();

    assert_eq!(
        RequesterXkey::parse(&seed).unwrap_err(),
        RequesterXkeyError::NotACurveKey
    );
}

#[test]
fn accepts_a_curve_public_key() {
    let public = XKey::new().public_key();

    let requester = RequesterXkey::parse(&public).unwrap();

    assert_eq!(requester.as_str(), public);
    assert_eq!(requester.key().public_key(), public);
    assert_eq!(requester.to_string(), public);
    assert_eq!(format!("{requester:?}"), format!("RequesterXkey({public:?})"));
}

#[test]
fn compares_by_the_encoded_key() {
    let public = XKey::new().public_key();
    let other = XKey::new().public_key();

    assert_eq!(
        RequesterXkey::parse(&public).unwrap(),
        RequesterXkey::parse(&public).unwrap()
    );
    assert_ne!(
        RequesterXkey::parse(&public).unwrap(),
        RequesterXkey::parse(&other).unwrap()
    );
}

#[test]
fn error_says_what_was_wrong() {
    assert_eq!(
        RequesterXkeyError::NotACurveKey.to_string(),
        "requester xkey is not a curve public key"
    );
}
