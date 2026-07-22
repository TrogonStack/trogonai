use super::*;
use crate::wire::NkeyPublic;
use nkeys::KeyPair;

#[test]
fn from_user_nkey_round_trips_display_and_nkey_accessor() {
    let raw = KeyPair::new_user().public_key();
    let nkey = NkeyPublic::parse(raw.clone()).unwrap();
    let subject = UserJwtSubject::from_user_nkey(nkey.clone());
    assert_eq!(subject.as_str(), raw);
    assert_eq!(subject.to_string(), raw);
    assert_eq!(subject.nkey().as_str(), raw);
    assert!(format!("{subject:?}").contains(&raw));
}
