use base64::Engine as _;
use nkeys::{KeyPair, XKey};
use serde_json::json;

use super::*;
use crate::wire::test_encode::FixtureServer;

fn callout_xkey() -> CalloutXkey {
    CalloutXkey::from_seed(&XKey::new().seed().unwrap()).unwrap()
}

fn decode_plaintext(server: &FixtureServer, token: String) -> Result<ServerAuthRequestClaims, AuthRequestError> {
    ServerAuthRequestEnvelope::from_bytes(token.into_bytes()).decode(server.auth_account(), None, None)
}

fn xkey_header(xkey: &str) -> async_nats::HeaderMap {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(AUTH_REQUEST_XKEY_HEADER, xkey);
    headers
}

#[test]
fn accepts_a_plaintext_request_a_server_signed_for_the_auth_account() {
    let server = FixtureServer::generate();
    let user = KeyPair::new_user();
    let decoded = decode_plaintext(&server, server.sign(&user, |_| {})).unwrap();
    assert_eq!(decoded.user_nkey().unwrap().as_str(), user.public_key());
    assert_eq!(decoded.server_id(), server.id());
    assert_eq!(decoded.server_one_time_xkey(), None);
}

#[test]
fn accepts_every_server_of_the_auth_account_whatever_key_it_booted_with() {
    let server = FixtureServer::generate();
    let peer = FixtureServer::signing_with(KeyPair::new_server(), server.auth_account().clone());
    let decoded = decode_plaintext(&server, peer.sign(&KeyPair::new_user(), |_| {})).unwrap();
    assert_eq!(decoded.server_id(), peer.id());
}

#[test]
fn accepts_an_encrypted_request_sealed_by_the_server_xkey_it_names() {
    let server = FixtureServer::generate();
    let callout = callout_xkey();
    let user = KeyPair::new_user();
    let (sealed, headers) = server.seal(&user, &callout.public_key(), |_| {});
    let decoded =
        ServerAuthRequestEnvelope::decode_from_message(sealed, Some(&headers), server.auth_account(), Some(&callout))
            .unwrap();
    assert_eq!(decoded.user_nkey().unwrap().as_str(), user.public_key());
    assert_eq!(
        decoded.server_one_time_xkey(),
        Some(server.xkey().public_key().as_str())
    );
}

#[test]
fn refuses_a_request_signed_by_a_key_that_is_not_a_server() {
    let server = FixtureServer::generate();
    let impostor = FixtureServer::signing_with(KeyPair::new_account(), server.auth_account().clone());
    let refusal = decode_plaintext(&server, impostor.sign(&KeyPair::new_user(), |_| {})).unwrap_err();
    assert!(matches!(refusal, AuthRequestError::IssuerNotAServer));
}

#[test]
fn refuses_claims_naming_a_server_whose_key_did_not_sign_them() {
    let server = FixtureServer::generate();
    let token = server.sign(&KeyPair::new_user(), |_| {});
    let (signed, _) = token.rsplit_once('.').unwrap();
    let forged = URL_SAFE_NO_PAD.encode(KeyPair::new_server().sign(signed.as_bytes()).unwrap());
    let refusal = decode_plaintext(&server, format!("{signed}.{forged}")).unwrap_err();
    assert!(matches!(refusal, AuthRequestError::BadSignature));
}

#[test]
fn refuses_a_request_issued_for_another_account() {
    let server = FixtureServer::generate();
    let elsewhere = FixtureServer::generate();
    let refusal = decode_plaintext(&server, elsewhere.sign(&KeyPair::new_user(), |_| {})).unwrap_err();
    assert!(matches!(refusal, AuthRequestError::WrongAccount));
}

#[test]
fn refuses_a_server_id_other_than_the_signing_key() {
    let server = FixtureServer::generate();
    let token = server.sign(&KeyPair::new_user(), |claims| {
        claims.nats.server.id = KeyPair::new_server().public_key();
    });
    let refusal = decode_plaintext(&server, token).unwrap_err();
    assert!(matches!(refusal, AuthRequestError::ServerIdMismatch));
}

#[test]
fn refuses_a_request_addressed_elsewhere() {
    let server = FixtureServer::generate();
    let token = server.sign(&KeyPair::new_user(), |claims| {
        claims.aud = Some("wrong-audience".into());
    });
    let refusal = decode_plaintext(&server, token).unwrap_err();
    assert!(matches!(refusal, AuthRequestError::WrongAudience));
}

#[test]
fn refuses_a_token_without_three_segments() {
    let server = FixtureServer::generate();
    let refusal = decode_plaintext(&server, "eyJhbGciOiJlZDI1NTE5In0.only-two".to_owned()).unwrap_err();
    assert!(matches!(refusal, AuthRequestError::NotAJwt));
}

#[test]
fn refuses_a_plaintext_request_that_asks_for_an_encrypted_response() {
    let server = FixtureServer::generate();
    let token = server.sign(&KeyPair::new_user(), |claims| {
        claims.nats.server.xkey = Some(XKey::new().public_key());
    });
    let refusal = decode_plaintext(&server, token).unwrap_err();
    assert!(matches!(refusal, AuthRequestError::PlaintextAsksForEncryption));
}

#[test]
fn refuses_a_plaintext_request_when_the_callout_expects_encryption() {
    let server = FixtureServer::generate();
    let token = server.sign(&KeyPair::new_user(), |_| {});
    let refusal = ServerAuthRequestEnvelope::from_bytes(token.into_bytes())
        .decode(server.auth_account(), Some(&callout_xkey()), None)
        .unwrap_err();
    assert!(matches!(refusal, AuthRequestError::UnexpectedPlaintext));
}

#[test]
fn refuses_an_encrypted_request_when_the_callout_has_no_xkey() {
    let server = FixtureServer::generate();
    let (sealed, headers) = server.seal(&KeyPair::new_user(), &callout_xkey().public_key(), |_| {});
    let refusal = ServerAuthRequestEnvelope::decode_from_message(sealed, Some(&headers), server.auth_account(), None)
        .unwrap_err();
    assert!(matches!(refusal, AuthRequestError::UnexpectedEncryption));
}

#[test]
fn refuses_an_encrypted_request_without_the_server_xkey_header() {
    let server = FixtureServer::generate();
    let callout = callout_xkey();
    let (sealed, _) = server.seal(&KeyPair::new_user(), &callout.public_key(), |_| {});
    let refusal = ServerAuthRequestEnvelope::decode_from_message(sealed, None, server.auth_account(), Some(&callout))
        .unwrap_err();
    assert!(matches!(refusal, AuthRequestError::MissingServerXkey));
}

#[test]
fn refuses_a_server_xkey_header_that_is_not_a_curve_key() {
    let server = FixtureServer::generate();
    let callout = callout_xkey();
    let (sealed, _) = server.seal(&KeyPair::new_user(), &callout.public_key(), |_| {});
    let refusal = ServerAuthRequestEnvelope::decode_from_message(
        sealed,
        Some(&xkey_header("not-an-xkey")),
        server.auth_account(),
        Some(&callout),
    )
    .unwrap_err();
    assert!(matches!(refusal, AuthRequestError::InvalidServerXkey));
}

#[test]
fn refuses_an_encrypted_request_that_the_header_xkey_did_not_seal() {
    let server = FixtureServer::generate();
    let callout = callout_xkey();
    let (sealed, _) = server.seal(&KeyPair::new_user(), &callout.public_key(), |_| {});
    let refusal = ServerAuthRequestEnvelope::decode_from_message(
        sealed,
        Some(&xkey_header(&XKey::new().public_key())),
        server.auth_account(),
        Some(&callout),
    )
    .unwrap_err();
    assert!(matches!(refusal, AuthRequestError::Unsealable(_)));
}

#[test]
fn refuses_an_encrypted_request_that_names_another_server_xkey() {
    let server = FixtureServer::generate();
    let callout = callout_xkey();
    let (sealed, headers) = server.seal(&KeyPair::new_user(), &callout.public_key(), |claims| {
        claims.nats.server.xkey = Some(XKey::new().public_key());
    });
    let refusal =
        ServerAuthRequestEnvelope::decode_from_message(sealed, Some(&headers), server.auth_account(), Some(&callout))
            .unwrap_err();
    assert!(matches!(refusal, AuthRequestError::ServerXkeyMismatch));
}

#[test]
fn normalize_fills_the_sections_a_server_may_omit() {
    let mut payload = json!({ "nats": { "client_info": {}, "client_tls": {} } });

    normalize_auth_request_payload(&mut payload);

    assert_eq!(payload["nats"]["client_info"]["name_tag"], json!(""));
    assert_eq!(payload["nats"]["client_info"]["nonce"], json!(""));
    assert_eq!(payload["nats"]["client_tls"]["certs"], json!([]));
    assert_eq!(payload["nats"]["client_tls"]["verified_chains"], json!([]));
}

#[test]
fn normalize_leaves_a_payload_without_the_sections_alone() {
    let mut payload = json!({ "nats": {} });

    normalize_auth_request_payload(&mut payload);

    assert_eq!(payload, json!({ "nats": {} }));
}

#[test]
fn from_bytes_and_as_bytes_roundtrip() {
    let bytes = b"payload".to_vec();
    let envelope = ServerAuthRequestEnvelope::from_bytes(bytes.clone());
    assert_eq!(envelope.as_bytes(), bytes.as_slice());
}
