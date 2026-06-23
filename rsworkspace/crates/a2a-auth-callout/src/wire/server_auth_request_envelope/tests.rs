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
    assert!(
        err.to_string().contains("decode")
            || err.to_string().contains("verify")
            || err.to_string().contains("signature")
    );
}
