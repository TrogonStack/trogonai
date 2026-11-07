use super::*;
use crate::fixtures::signing_key;

fn key_id(seed: u8) -> KeyId {
    KeyId::from_verifying_key(signing_key(seed).verifying_key())
}

#[test]
fn trusts_only_the_keys_it_holds() {
    let trusted = TrustedKeys::new([key_id(1), key_id(2)]);

    assert!(trusted.contains(&key_id(1)));
    assert!(trusted.contains(&key_id(2)));
    assert!(!trusted.contains(&key_id(3)));
    assert!(!TrustedKeys::default().contains(&key_id(1)));
}
