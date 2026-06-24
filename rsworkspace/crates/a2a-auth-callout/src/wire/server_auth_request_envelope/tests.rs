use super::*;
use crate::wire::test_encode::signed_auth_request;
use nkeys::{KeyPair, XKey};

#[test]
fn roundtrip_plain_jwt_request() {
    let server = KeyPair::new_account();
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    let user = KeyPair::new_user();
    let token = signed_auth_request(&server, &user, |_| {});
    let decoded = ServerAuthRequestEnvelope::from_bytes(token.into_bytes())
        .decode(&server_pub, None, None)
        .unwrap();
    assert_eq!(decoded.user_nkey().unwrap().as_str(), user.public_key());
}

#[test]
fn xkey_encrypted_request_roundtrip() {
    let server = KeyPair::new_account();
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    let account_xkey = XKey::new();
    let account_seed = NkeySeed::parse(account_xkey.seed().unwrap()).unwrap();

    let server_xkey = XKey::new();
    let server_xkey_pub = XkeyPublic::parse(server_xkey.public_key()).unwrap();
    let user = KeyPair::new_user();
    let token = signed_auth_request(&server, &user, |_| {});
    let sealed = server_xkey.seal(token.as_bytes(), &account_xkey).unwrap();

    let decoded = ServerAuthRequestEnvelope::from_bytes(sealed)
        .decode(&server_pub, Some(&account_seed), Some(&server_xkey_pub))
        .unwrap();
    assert_eq!(decoded.user_nkey().unwrap().as_str(), user.public_key());
}

#[test]
fn rejects_tampered_signature() {
    let server = KeyPair::new_account();
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    let user = KeyPair::new_user();
    let mut token = signed_auth_request(&server, &user, |_| {});
    let last = token.pop().unwrap();
    token.push(if last == 'a' { 'b' } else { 'a' });
    let err = ServerAuthRequestEnvelope::from_bytes(token.into_bytes())
        .decode(&server_pub, None, None)
        .unwrap_err();
    assert!(matches!(err, AuthCalloutError::WireFormat(_)));
}

#[test]
fn rejects_plain_jwt_when_account_xkey_configured() {
    let server = KeyPair::new_account();
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    let account_xkey = XKey::new();
    let account_seed = NkeySeed::parse(account_xkey.seed().unwrap()).unwrap();
    let user = KeyPair::new_user();
    let token = signed_auth_request(&server, &user, |_| {});

    let err = ServerAuthRequestEnvelope::from_bytes(token.into_bytes())
        .decode(&server_pub, Some(&account_seed), None)
        .unwrap_err();
    assert!(matches!(err, AuthCalloutError::WireFormat(_)));
}

#[test]
fn encrypted_request_requires_account_xkey_seed() {
    let server = KeyPair::new_account();
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    let err = ServerAuthRequestEnvelope::from_bytes(b"not-a-jwt".to_vec())
        .decode(&server_pub, None, None)
        .unwrap_err();
    assert!(matches!(err, AuthCalloutError::WireFormat(_)));
}

#[test]
fn encrypted_request_requires_server_xkey_public() {
    let server = KeyPair::new_account();
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    let account_xkey = XKey::new();
    let account_seed = NkeySeed::parse(account_xkey.seed().unwrap()).unwrap();

    let err = ServerAuthRequestEnvelope::from_bytes(b"not-a-jwt".to_vec())
        .decode(&server_pub, Some(&account_seed), None)
        .unwrap_err();
    assert!(matches!(err, AuthCalloutError::WireFormat(_)));
}

#[test]
fn decode_from_message_prefers_header_server_xkey() {
    let server = KeyPair::new_account();
    let server_pub = NkeyPublic::parse(server.public_key()).unwrap();
    let account_xkey = XKey::new();
    let account_seed = NkeySeed::parse(account_xkey.seed().unwrap()).unwrap();
    let header_xkey = XKey::new();
    let configured_xkey = XKey::new();
    let configured_pub = XkeyPublic::parse(configured_xkey.public_key()).unwrap();
    let user = KeyPair::new_user();
    let token = signed_auth_request(&server, &user, |_| {});
    let sealed = header_xkey.seal(token.as_bytes(), &account_xkey).unwrap();

    let mut headers = async_nats::HeaderMap::new();
    headers.insert(
        super::super::AUTH_REQUEST_XKEY_HEADER,
        header_xkey.public_key(),
    );

    let decoded = ServerAuthRequestEnvelope::decode_from_message(
        sealed,
        Some(&headers),
        &server_pub,
        Some(&account_seed),
        Some(&configured_pub),
    )
    .unwrap();
    assert_eq!(decoded.user_nkey().unwrap().as_str(), user.public_key());
}

#[test]
fn from_bytes_and_as_bytes_roundtrip() {
    let bytes = b"payload".to_vec();
    let envelope = ServerAuthRequestEnvelope::from_bytes(bytes.clone());
    assert_eq!(envelope.as_bytes(), bytes.as_slice());
}
