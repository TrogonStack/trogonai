use base64::Engine as _;

use super::*;
use crate::wire::{NATS_JWT_PREFIX, ServerAuthRequestEnvelope, test_encode::FixtureServer};
use nkeys::{KeyPair, XKey};

fn response_signing_key() -> AccountSigningKey {
    AccountSigningKey::from_seed(&KeyPair::new_account().seed().unwrap()).unwrap()
}

fn fixture_request() -> (ServerAuthRequestClaims, AccountSigningKey) {
    let request = FixtureServer::generate().request(&KeyPair::new_user(), |c| {
        c.nats.connect_opts.jwt = None;
    });
    (request, response_signing_key())
}

#[test]
fn success_response_roundtrip_fields() {
    let (req, callout) = fixture_request();
    let resp = CalloutAuthResponseClaims::success(
        &req,
        &MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ4In0.c2ln").unwrap(),
        &callout,
    )
    .unwrap();
    // `nats-jwt-rs` omits empty `nats.error` on encode; Go accepts that, but Rust decode requires the field.
    let parts: Vec<&str> = resp.as_jwt_str().split('.').collect();
    assert_eq!(parts.len(), 3);
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .expect("response JWT payload base64");
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes).expect("response JWT payload JSON");
    assert_eq!(payload["sub"], req.user_nkey().unwrap().as_str());
    assert_eq!(payload["aud"], req.server_id());
    assert_eq!(
        payload["nats"]["jwt"].as_str().unwrap(),
        "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ4In0.c2ln"
    );
    assert!(
        payload["nats"]["error"].as_str().unwrap_or("").is_empty(),
        "success response must not set nats.error"
    );
}

#[test]
fn denial_response_carries_error_claim() {
    let (req, callout) = fixture_request();
    let resp = CalloutAuthResponseClaims::denial(&req, "authorization denied", &callout).unwrap();
    let decoded = nats_jwt_rs::Claims::<AuthResponse>::decode(resp.as_jwt_str()).unwrap();
    assert_eq!(decoded.nats.error, "authorization denied");
    assert!(decoded.nats.jwt.is_empty());
}

#[test]
fn into_wire_bytes_returns_plain_jwt_without_server_xkey() {
    let (req, callout) = fixture_request();
    let resp = CalloutAuthResponseClaims::success(
        &req,
        &MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ4In0.c2ln").unwrap(),
        &callout,
    )
    .unwrap();
    let wire = resp.into_wire_bytes(&req, None).unwrap();
    assert!(wire.starts_with(NATS_JWT_PREFIX));
}

#[test]
fn into_wire_bytes_encrypts_when_server_requests_xkey() {
    let server = FixtureServer::generate();
    let callout = response_signing_key();
    let account_xkey = XKey::new();
    let callout_xkey = CalloutXkey::from_seed(&account_xkey.seed().unwrap()).unwrap();
    let (sealed, headers) = server.seal(&KeyPair::new_user(), &callout_xkey.public_key(), |_| {});
    let req = ServerAuthRequestEnvelope::decode_from_message(
        sealed,
        Some(&headers),
        server.auth_account(),
        Some(&callout_xkey),
    )
    .unwrap();
    let resp = CalloutAuthResponseClaims::success(
        &req,
        &MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ4In0.c2ln").unwrap(),
        &callout,
    )
    .unwrap();
    let wire = resp.into_wire_bytes(&req, Some(&callout_xkey)).unwrap();
    assert!(!wire.starts_with(NATS_JWT_PREFIX));
    let opened = server.xkey().open(&wire, &account_xkey).unwrap();
    assert!(opened.starts_with(NATS_JWT_PREFIX));
}

#[test]
fn into_wire_bytes_errors_when_server_xkey_but_no_callout_xkey() {
    let callout = response_signing_key();
    let claims = FixtureServer::generate().claims(&KeyPair::new_user(), |c| {
        c.nats.server.xkey = Some(XKey::new().public_key());
    });
    let req = ServerAuthRequestClaims::from_decoded(claims, serde_json::json!({}));
    let resp = CalloutAuthResponseClaims::denial(&req, "no", &callout).unwrap();
    let err = resp.into_wire_bytes(&req, None).unwrap_err();
    assert!(matches!(err, AuthCalloutError::WireFormat(_)));
}

#[test]
fn debug_does_not_leak_the_encoded_response() {
    let (req, callout) = fixture_request();
    let resp = CalloutAuthResponseClaims::success(
        &req,
        &MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ4In0.c2ln").unwrap(),
        &callout,
    )
    .unwrap();
    let dbg = format!("{resp:?}");
    assert!(!dbg.contains(resp.as_jwt_str()), "{dbg}");
    assert!(dbg.contains("<redacted>"), "{dbg}");
}
