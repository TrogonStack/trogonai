use super::*;
use crate::jwt::MintedUserJwt;
use crate::wire::{NATS_JWT_PREFIX, test_encode::signed_auth_request};
use nats_jwt_rs::authorization::AuthResponse;
use nkeys::{KeyPair, XKey};

fn signing_key(key: &KeyPair) -> AccountSigningKey {
    AccountSigningKey::from_seed(&key.seed().unwrap()).unwrap()
}

fn callout_xkey(key: &XKey) -> CalloutXkey {
    CalloutXkey::from_seed(&key.seed().unwrap()).unwrap()
}

#[test]
fn rejects_partial_xkey_configuration() {
    let server = KeyPair::new_account();
    let callout = KeyPair::new_account();
    let account_xkey = XKey::new();
    let server_xkey_pub = XkeyPublic::parse(XKey::new().public_key()).unwrap();

    let missing_server = AuthCalloutWireCodec::new(
        NkeyPublic::parse(server.public_key()).unwrap(),
        signing_key(&callout),
        Some(callout_xkey(&account_xkey)),
        None,
    );
    assert!(matches!(missing_server.unwrap_err(), AuthCalloutError::WireFormat(_)));

    let missing_account = AuthCalloutWireCodec::new(
        NkeyPublic::parse(server.public_key()).unwrap(),
        signing_key(&callout),
        None,
        Some(server_xkey_pub),
    );
    assert!(matches!(missing_account.unwrap_err(), AuthCalloutError::WireFormat(_)));
}

#[test]
fn decode_and_encode_success_roundtrip() {
    let server = KeyPair::new_account();
    let callout = KeyPair::new_account();
    let user = KeyPair::new_user();
    let codec = AuthCalloutWireCodec::new(
        NkeyPublic::parse(server.public_key()).unwrap(),
        signing_key(&callout),
        None,
        None,
    )
    .unwrap();

    let token = signed_auth_request(&server, &user, |_| {});
    let request = codec.decode_request(token.into_bytes(), None).unwrap();
    let user_jwt = MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ4In0.c2ln").unwrap();
    let wire = codec.encode_success(&request, user_jwt).unwrap();
    assert!(wire.starts_with(NATS_JWT_PREFIX));
}

#[test]
fn encode_denial_is_signed_by_the_response_signing_key() {
    let server = KeyPair::new_account();
    let callout = KeyPair::new_account();
    let user = KeyPair::new_user();
    let codec = AuthCalloutWireCodec::new(
        NkeyPublic::parse(server.public_key()).unwrap(),
        signing_key(&callout),
        None,
        None,
    )
    .unwrap();
    let token = signed_auth_request(&server, &user, |_| {});
    let request = codec.decode_request(token.into_bytes(), None).unwrap();
    let wire = codec.encode_denial(&request, "denied").unwrap();

    let decoded = nats_jwt_rs::Claims::<AuthResponse>::decode(std::str::from_utf8(&wire).unwrap()).unwrap();
    assert_eq!(decoded.iss, callout.public_key());
    assert_eq!(decoded.nats.error, "denied");
}

#[test]
fn xkey_codec_roundtrip_encrypts_request_and_response() {
    let server = KeyPair::new_account();
    let callout = KeyPair::new_account();
    let account_xkey = XKey::new();
    let server_xkey = XKey::new();
    let server_xkey_pub = XkeyPublic::parse(server_xkey.public_key()).unwrap();
    let codec = AuthCalloutWireCodec::new(
        NkeyPublic::parse(server.public_key()).unwrap(),
        signing_key(&callout),
        Some(callout_xkey(&account_xkey)),
        Some(server_xkey_pub),
    )
    .unwrap();

    let user = KeyPair::new_user();
    let token = signed_auth_request(&server, &user, |c| {
        c.nats.server.xkey = Some(server_xkey.public_key());
    });
    let sealed = server_xkey.seal(token.as_bytes(), &account_xkey).unwrap();
    let request = codec.decode_request(sealed, None).unwrap();
    let user_jwt = MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ4In0.c2ln").unwrap();
    let wire = codec.encode_success(&request, user_jwt).unwrap();
    assert!(!wire.starts_with(NATS_JWT_PREFIX));
    let opened = server_xkey.open(&wire, &account_xkey).unwrap();
    assert!(opened.starts_with(NATS_JWT_PREFIX));
}

#[test]
fn debug_names_public_keys_and_hides_seeds() {
    let server = KeyPair::new_account();
    let callout = KeyPair::new_account();
    let account_xkey = XKey::new();
    let codec = AuthCalloutWireCodec::new(
        NkeyPublic::parse(server.public_key()).unwrap(),
        signing_key(&callout),
        Some(callout_xkey(&account_xkey)),
        Some(XkeyPublic::parse(XKey::new().public_key()).unwrap()),
    )
    .unwrap();
    let debug = format!("{codec:?}");
    assert!(debug.contains(&callout.public_key()), "{debug}");
    assert!(debug.contains(&account_xkey.public_key()), "{debug}");
    assert!(
        !debug.contains(&callout.seed().unwrap()),
        "response signing seed leaked in Debug output"
    );
    assert!(
        !debug.contains(&account_xkey.seed().unwrap()),
        "callout xkey seed leaked in Debug output"
    );
}
