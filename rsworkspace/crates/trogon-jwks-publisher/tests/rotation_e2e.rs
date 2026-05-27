//! Offline rotation overlap: file-PEM signer, mocked publication surfaces (KV bytes, HTTPS, req-reply).

use std::collections::HashMap;
use std::time::Duration;

use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, decode_header, encode,
};
use rand::rngs::OsRng;
use rsa::pkcs8::EncodePrivateKey;
use rsa::RsaPrivateKey;
use tempfile::tempdir;
use trogon_jwks_publisher::JwksState;
use trogon_jwks_publisher::jwks::Jwks;
use trogon_jwks_publisher::keys::file::{load_jwks_from_dir, FileKeySource};
use trogon_jwks_publisher::publishers::kv::{kid_kv_key, serialize_jwks};
use trogon_jwks_publisher::publishers::reqrep::{ReqRepState, current_jwks_json, set_jwks};
use trogon_jwks_publisher::signer::file::FileMeshSigner;
use trogon_jwks_publisher::signer::{
    DEFAULT_MESH_ISSUER, MAX_MESH_TOKEN_TTL_SECS, MIN_ROTATION_OVERLAP_SECS, MeshSigner, mesh_claims,
};
use trogon_jwks_publisher::{DEFAULT_JWT_LEEWAY_SECS, KeySource};

fn write_rsa_pem(dir: &std::path::Path, name: &str) {
    let key = RsaPrivateKey::new(&mut OsRng, 2048).expect("rsa key");
    let pem = key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).expect("pem");
    std::fs::write(dir.join(name), pem).expect("write pem");
}

struct MockKv {
    current: Vec<u8>,
    per_kid: HashMap<String, Vec<u8>>,
}

impl MockKv {
    fn publish(&mut self, jwks: &Jwks) {
        let payload = serialize_jwks(jwks).expect("serialize");
        self.current = payload.clone();
        self.per_kid.clear();
        for kid in jwks.kids() {
            self.per_kid.insert(kid_kv_key(kid), payload.clone());
        }
    }
}

fn assert_channels_match(jwks: &Jwks, kv: &MockKv, reqrep: &ReqRepState, https_cache: &Jwks) {
    let kv_jwks: Jwks = serde_json::from_slice(&kv.current).expect("kv json");
    let reqrep_jwks: Jwks =
        serde_json::from_slice(&current_jwks_json(reqrep).expect("reqrep json")).expect("reqrep parse");
    assert_eq!(kv_jwks, *jwks);
    assert_eq!(reqrep_jwks, *jwks);
    assert_eq!(https_cache, jwks);
    assert_eq!(kv.per_kid.len(), jwks.keys.len());
}

fn verify_with_jwks(jwks: &Jwks, token: &str, expected_iss: &str, expected_kid: &str) {
    let header = decode_header(token).expect("header");
    assert_eq!(header.kid.as_deref(), Some(expected_kid));
    let set = jwks.to_jwk_set();
    let jwk = set.find(expected_kid).expect("jwk for kid");
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[expected_iss]);
    validation.validate_exp = false;
    let key = DecodingKey::from_jwk(jwk).expect("decoding key");
    let token_data = decode::<HashMap<String, serde_json::Value>>(token, &key, &validation).expect("verify");
    assert_eq!(
        token_data.claims.get("iss").and_then(|v| v.as_str()),
        Some(expected_iss)
    );
}

fn sign_with_pem(pem: &[u8], kid: &str, issuer: &str) -> String {
    let encoding_key = EncodingKey::from_rsa_pem(pem).expect("encoding key");
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.into());
    header.typ = Some("JWT".into());
    let claims = mesh_claims(issuer, "urn:trogon:mesh:agent:acme/demo", 120);
    encode(&header, &claims, &encoding_key).expect("encode")
}

#[tokio::test]
async fn rotation_overlap_publishes_to_all_channels_and_verifies_retired_kid() {
    let dir = tempdir().expect("tempdir");
    write_rsa_pem(dir.path(), "previous.pem");
    let retired_kid = load_jwks_from_dir(dir.path()).expect("previous jwks").keys[0]
        .common
        .key_id
        .clone()
        .expect("retired kid");
    write_rsa_pem(dir.path(), "current.pem");
    let previous_pem = std::fs::read(dir.path().join("previous.pem")).expect("previous bytes");

    let (source, _watch_rx) = FileKeySource::spawn(dir.path().to_path_buf())
        .await
        .expect("spawn file source");
    let overlap = source.current().await.expect("overlap jwks");
    assert_eq!(overlap.keys.len(), 2);

    let signer = FileMeshSigner::from_key_dir(dir.path(), None).expect("file signer");
    assert_eq!(signer.mesh_issuer(), DEFAULT_MESH_ISSUER);

    let (state, _) = JwksState::new(overlap.clone());
    let reqrep = ReqRepState::new(overlap.clone());
    let https_cache = overlap.clone();
    let mut kv = MockKv {
        current: Vec::new(),
        per_kid: HashMap::new(),
    };
    kv.publish(&overlap);
    assert_channels_match(&overlap, &kv, &reqrep, &https_cache);

    let claims = mesh_claims(signer.mesh_issuer(), "urn:trogon:mesh:agent:acme/demo", 120);
    let token = signer.sign(&claims).await.expect("sign");
    verify_with_jwks(&overlap, &token, DEFAULT_MESH_ISSUER, signer.current_kid());

    let retired_token = sign_with_pem(&previous_pem, &retired_kid, DEFAULT_MESH_ISSUER);

    std::fs::remove_file(dir.path().join("previous.pem")).expect("retire previous");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut retired_overlap = overlap.clone();
    retired_overlap.keys.retain(|key| key.common.key_id.as_deref() == Some(retired_kid.as_str()));
    assert_eq!(retired_overlap.keys.len(), 1);

    state.update(retired_overlap.clone());
    set_jwks(&reqrep, retired_overlap.clone()).await;
    let https_cache = retired_overlap.clone();
    kv.publish(&retired_overlap);
    assert_channels_match(&retired_overlap, &kv, &reqrep, &https_cache);

    verify_with_jwks(&retired_overlap, &retired_token, DEFAULT_MESH_ISSUER, &retired_kid);

    let active_only = load_jwks_from_dir(dir.path()).expect("active only");
    assert_eq!(active_only.keys.len(), 1);
    state.update(active_only.clone());
    set_jwks(&reqrep, active_only.clone()).await;
    let https_cache = active_only.clone();
    kv.publish(&active_only);
    assert_channels_match(&active_only, &kv, &reqrep, &https_cache);

    let active_set = active_only.to_jwk_set();
    assert!(active_set.find(&retired_kid).is_none());
    let header = decode_header(&retired_token).expect("header");
    assert!(
        active_set
            .find(header.kid.as_deref().unwrap_or_default())
            .is_none()
    );

    const _: () = assert!(MIN_ROTATION_OVERLAP_SECS >= MAX_MESH_TOKEN_TTL_SECS + DEFAULT_JWT_LEEWAY_SECS);
}
