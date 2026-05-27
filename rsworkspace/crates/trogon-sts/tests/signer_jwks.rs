//! Asserts file-PEM signer `kid`/`iss` align with a JWKS publisher fixture and minted tokens verify.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use rsa::RsaPrivateKey;
use rsa::pkcs8::{EncodePrivateKey, LineEnding};
use rsa::traits::PublicKeyParts;
use serde_json::{Value, json};
use trogon_sts::signer::{DynSigner, FileSigner};

const FIXTURE_KID: &str = "mesh-publisher-fixture";
const FIXTURE_ISS: &str = "https://sts.trogon.ai/acme";

fn fixture_keypair() -> (String, JwkSet) {
    let mut rng = rand::thread_rng();
    let private = RsaPrivateKey::new(&mut rng, 2048).expect("rsa");
    let pem = private
        .to_pkcs8_pem(LineEnding::LF)
        .expect("pem")
        .to_string();
    let public = private.to_public_key();
    let n = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
    let e = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());
    let jwks: JwkSet = serde_json::from_value(json!({
        "keys": [{
            "kty": "RSA",
            "kid": FIXTURE_KID,
            "use": "sig",
            "alg": "RS256",
            "n": n,
            "e": e
        }]
    }))
    .expect("jwks");
    (pem, jwks)
}

#[tokio::test]
async fn file_signer_kid_and_iss_match_publisher_fixture_and_token_verifies() {
    let (pem, fixture_jwks) = fixture_keypair();
    let signer: DynSigner = Arc::new(
        FileSigner::from_rsa_pem(&pem, FIXTURE_KID).expect("file signer"),
    );

    assert_eq!(signer.current_kid(), FIXTURE_KID);
    assert_eq!(
        fixture_jwks.keys[0].common.key_id.as_deref(),
        Some(FIXTURE_KID)
    );

    let mut claims: HashMap<String, Value> = HashMap::new();
    claims.insert("iss".into(), json!(FIXTURE_ISS));
    claims.insert("sub".into(), json!("agent:acme/oncall-agent"));
    claims.insert("aud".into(), json!("urn:trogon:a2a:agent:acme:oncall-agent"));
    claims.insert("iat".into(), json!(1_748_347_200_i64));
    claims.insert("exp".into(), json!(1_748_347_320_i64));

    let token = signer.sign(&claims).await.expect("sign");
    let header = decode_header(&token).expect("header");
    assert_eq!(header.kid.as_deref(), Some(FIXTURE_KID));

    let payload = decode_payload(&token);
    assert_eq!(payload.get("iss").and_then(Value::as_str), Some(FIXTURE_ISS));

    let mesh_jwk = fixture_jwks.keys.first().expect("fixture jwk");
    let dec = DecodingKey::from_jwk(mesh_jwk).expect("dec key");
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[FIXTURE_ISS]);
    validation.validate_aud = false;
    validation.validate_exp = false;
    decode::<Value>(&token, &dec, &validation).expect("mesh jwt verifies against publisher JWKS");
}

fn decode_payload(token: &str) -> Value {
    let payload = token.split('.').nth(1).expect("jwt parts");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("b64");
    serde_json::from_slice(&bytes).expect("json")
}
