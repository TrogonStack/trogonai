use super::*;
use crate::jwt::MintedUserJwt;
use crate::wire::{NATS_JWT_PREFIX, test_encode::FixtureServer};
use nats_jwt_rs::authorization::AuthResponse;
use nkeys::{KeyPair, XKey};

const MINTED: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ4In0.c2ln";

fn signing_key(key: &KeyPair) -> AccountSigningKey {
    AccountSigningKey::from_seed(&key.seed().unwrap()).unwrap()
}

fn callout_xkey(key: &XKey) -> CalloutXkey {
    CalloutXkey::from_seed(&key.seed().unwrap()).unwrap()
}

#[test]
fn decode_and_encode_success_roundtrip() {
    let server = FixtureServer::generate();
    let callout = KeyPair::new_account();
    let user = KeyPair::new_user();
    let codec = AuthCalloutWireCodec::new(server.auth_account().clone(), signing_key(&callout), None);

    let token = server.sign(&user, |_| {});
    let request = codec.decode_request(token.into_bytes(), None).unwrap();
    let wire = codec
        .encode_success(&request, MintedUserJwt::new(MINTED).unwrap())
        .unwrap();
    assert!(wire.starts_with(NATS_JWT_PREFIX));
}

#[test]
fn decode_request_refuses_a_request_issued_for_another_account() {
    let server = FixtureServer::generate();
    let elsewhere = FixtureServer::generate();
    let codec = AuthCalloutWireCodec::new(
        server.auth_account().clone(),
        signing_key(&KeyPair::new_account()),
        None,
    );

    let token = elsewhere.sign(&KeyPair::new_user(), |_| {});
    let refusal = codec.decode_request(token.into_bytes(), None).unwrap_err();
    assert!(matches!(refusal, AuthRequestError::WrongAccount));
}

#[test]
fn encode_denial_is_signed_by_the_response_signing_key() {
    let server = FixtureServer::generate();
    let callout = KeyPair::new_account();
    let user = KeyPair::new_user();
    let codec = AuthCalloutWireCodec::new(server.auth_account().clone(), signing_key(&callout), None);
    let token = server.sign(&user, |_| {});
    let request = codec.decode_request(token.into_bytes(), None).unwrap();
    let wire = codec.encode_denial(&request, "denied").unwrap();

    let decoded = nats_jwt_rs::Claims::<AuthResponse>::decode(std::str::from_utf8(&wire).unwrap()).unwrap();
    assert_eq!(decoded.iss, callout.public_key());
    assert_eq!(decoded.aud.as_deref(), Some(server.id().as_str()));
    assert_eq!(decoded.nats.error, "denied");
}

#[test]
fn xkey_codec_roundtrip_encrypts_request_and_response() {
    let server = FixtureServer::generate();
    let callout = KeyPair::new_account();
    let account_xkey = XKey::new();
    let codec = AuthCalloutWireCodec::new(
        server.auth_account().clone(),
        signing_key(&callout),
        Some(callout_xkey(&account_xkey)),
    );

    let user = KeyPair::new_user();
    let (sealed, headers) = server.seal(&user, &callout_xkey(&account_xkey).public_key(), |_| {});
    let request = codec.decode_request(sealed, Some(&headers)).unwrap();
    let wire = codec
        .encode_success(&request, MintedUserJwt::new(MINTED).unwrap())
        .unwrap();
    assert!(!wire.starts_with(NATS_JWT_PREFIX));
    let opened = server.xkey().open(&wire, &account_xkey).unwrap();
    assert!(opened.starts_with(NATS_JWT_PREFIX));
}

#[test]
fn debug_names_public_keys_and_hides_seeds() {
    let server = FixtureServer::generate();
    let callout = KeyPair::new_account();
    let account_xkey = XKey::new();
    let codec = AuthCalloutWireCodec::new(
        server.auth_account().clone(),
        signing_key(&callout),
        Some(callout_xkey(&account_xkey)),
    );
    let debug = format!("{codec:?}");
    assert!(debug.contains(server.auth_account().as_str()), "{debug}");
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
