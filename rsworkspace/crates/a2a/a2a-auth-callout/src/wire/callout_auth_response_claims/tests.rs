use base64::Engine as _;

use super::*;
use crate::wire::{NATS_JWT_PREFIX, NkeyPublic, ServerAuthRequestEnvelope, test_encode::signed_auth_request};
use nkeys::{KeyPair, XKey};

fn fixture_request() -> (ServerAuthRequestClaims, KeyPair) {
    let server = KeyPair::new_account();
    let callout = KeyPair::new_account();
    let user = KeyPair::new_user();
    let token = signed_auth_request(&server, &user, |c| {
        c.nats.connect_opts.jwt = None;
    });
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    let decoded = ServerAuthRequestEnvelope::from_bytes(token.into_bytes())
        .decode(&server_pub, None, None)
        .unwrap();
    (decoded, callout)
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
    let server = KeyPair::new_account();
    let callout = KeyPair::new_account();
    let account_xkey = XKey::new();
    let server_xkey = XKey::new();
    let user = KeyPair::new_user();
    let token = signed_auth_request(&server, &user, |c| {
        c.nats.server.xkey = Some(server_xkey.public_key());
    });
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    let req = ServerAuthRequestEnvelope::from_bytes(token.into_bytes())
        .decode(&server_pub, None, None)
        .unwrap();
    let resp = CalloutAuthResponseClaims::success(
        &req,
        &MintedUserJwt::new("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ4In0.c2ln").unwrap(),
        &callout,
    )
    .unwrap();
    let wire = resp.into_wire_bytes(&req, Some(&account_xkey)).unwrap();
    assert!(!wire.starts_with(NATS_JWT_PREFIX));
    let opened = account_xkey.open(&wire, &server_xkey).unwrap();
    assert!(opened.starts_with(NATS_JWT_PREFIX));
}

#[test]
fn into_wire_bytes_errors_when_server_xkey_but_no_account_xkey() {
    let server = KeyPair::new_account();
    let callout = KeyPair::new_account();
    let server_xkey = XKey::new();
    let user = KeyPair::new_user();
    let token = signed_auth_request(&server, &user, |c| {
        c.nats.server.xkey = Some(server_xkey.public_key());
    });
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    let req = ServerAuthRequestEnvelope::from_bytes(token.into_bytes())
        .decode(&server_pub, None, None)
        .unwrap();
    let resp = CalloutAuthResponseClaims::denial(&req, "no", &callout).unwrap();
    let err = resp.into_wire_bytes(&req, None).unwrap_err();
    assert!(matches!(err, AuthCalloutError::WireFormat(_)));
}
