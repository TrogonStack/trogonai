use super::*;

#[test]
fn is_the_lowercase_hex_sha256_of_the_token() {
    assert_eq!(
        PlaidDeliveryId::of("abc").as_str(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn the_same_token_always_gets_the_same_id() {
    assert_eq!(
        PlaidDeliveryId::of("header.claims.signature"),
        PlaidDeliveryId::of("header.claims.signature")
    );
}

#[test]
fn tokens_that_differ_only_in_signature_get_distinct_ids() {
    assert_ne!(
        PlaidDeliveryId::of("header.claims.signature-a"),
        PlaidDeliveryId::of("header.claims.signature-b")
    );
}
