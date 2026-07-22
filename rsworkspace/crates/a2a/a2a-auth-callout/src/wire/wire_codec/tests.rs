use super::*;
use crate::jwt::MintedUserJwt;
use crate::wire::{NATS_JWT_PREFIX, test_encode::signed_auth_request};
use nkeys::{KeyPair, XKey};

#[test]
fn rejects_partial_xkey_configuration() {
    let server = KeyPair::new_account();
    let callout = KeyPair::new_account();
    let account_xkey = XKey::new();
    let account_seed = NkeySeed::parse(account_xkey.seed().unwrap()).unwrap();
    let server_xkey_pub = XkeyPublic::parse(XKey::new().public_key()).unwrap();

    let missing_server = AuthCalloutWireCodec::new(
        NkeyPublic::parse(server.public_key()).unwrap(),
        NkeySeed::parse(callout.seed().unwrap()).unwrap(),
        Some(account_seed.clone()),
        None,
    );
    assert!(matches!(missing_server.unwrap_err(), AuthCalloutError::WireFormat(_)));

    let missing_account = AuthCalloutWireCodec::new(
        NkeyPublic::parse(server.public_key()).unwrap(),
        NkeySeed::parse(callout.seed().unwrap()).unwrap(),
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
        NkeySeed::parse(callout.seed().unwrap()).unwrap(),
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
fn encode_denial_returns_jwt_bytes() {
    let server = KeyPair::new_account();
    let callout = KeyPair::new_account();
    let user = KeyPair::new_user();
    let codec = AuthCalloutWireCodec::new(
        NkeyPublic::parse(server.public_key()).unwrap(),
        NkeySeed::parse(callout.seed().unwrap()).unwrap(),
        None,
        None,
    )
    .unwrap();
    let token = signed_auth_request(&server, &user, |_| {});
    let request = codec.decode_request(token.into_bytes(), None).unwrap();
    let wire = codec.encode_denial(&request, "denied").unwrap();
    assert!(wire.starts_with(NATS_JWT_PREFIX));
}

#[test]
fn xkey_codec_roundtrip_encrypts_request_and_response() {
    let server = KeyPair::new_account();
    let callout = KeyPair::new_account();
    let account_xkey = XKey::new();
    let account_seed = NkeySeed::parse(account_xkey.seed().unwrap()).unwrap();
    let server_xkey = XKey::new();
    let server_xkey_pub = XkeyPublic::parse(server_xkey.public_key()).unwrap();
    let codec = AuthCalloutWireCodec::new(
        NkeyPublic::parse(server.public_key()).unwrap(),
        NkeySeed::parse(callout.seed().unwrap()).unwrap(),
        Some(account_seed),
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
    let opened = account_xkey.open(&wire, &server_xkey).unwrap();
    assert!(opened.starts_with(NATS_JWT_PREFIX));
}

#[test]
fn debug_redacts_callout_seed() {
    let server = KeyPair::new_account();
    let callout = KeyPair::new_account();
    let seed = callout.seed().unwrap();
    let codec = AuthCalloutWireCodec::new(
        NkeyPublic::parse(server.public_key()).unwrap(),
        NkeySeed::parse(seed.clone()).unwrap(),
        None,
        None,
    )
    .unwrap();
    let debug = format!("{codec:?}");
    assert!(debug.contains("AuthCalloutWireCodec"));
    assert!(!debug.contains(&seed), "callout issuer seed leaked in Debug output");
}
