//! Shared test fixtures for STS exchange tests.

use std::sync::{Arc, LazyLock};

use base64::Engine;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::RsaPrivateKey;
use rsa::pkcs8::EncodePrivateKey;
use rsa::traits::PublicKeyParts;
use serde_json::{Value, json};
use trogon_sts::DEFAULT_MESH_ISSUER;
use trogon_sts::audit::RecordingAuditPublisher;
use trogon_sts::cache::{JwksCache, RegistryCache, TrustBundleCache};
use trogon_sts::exchange::ExchangeService;
use trogon_sts::registry::{AgentRegistryRecord, InMemoryRegistry};
use trogon_sts::signer::{DynSigner, FileSigner};
use trogon_sts::types::StsExchangeRequest;

pub struct TestKeys {
    pub bootstrap_jwks: JwkSet,
    pub mesh_jwks: JwkSet,
    pub bootstrap_enc: EncodingKey,
    pub mesh_signer: DynSigner,
    pub bootstrap_iss: String,
}

static SHARED_KEYS: LazyLock<TestKeys> = LazyLock::new(test_keys);

pub fn shared_test_keys() -> &'static TestKeys {
    &SHARED_KEYS
}

pub fn test_keys() -> TestKeys {
    let bootstrap = rsa_keypair("bootstrap-kid");
    let mesh = rsa_keypair("mesh-kid");
    TestKeys {
        bootstrap_jwks: bootstrap.jwks,
        bootstrap_enc: bootstrap.enc,
        mesh_jwks: mesh.jwks.clone(),
        mesh_signer: Arc::new(FileSigner::from_rsa_pem(mesh.pem.as_str(), "mesh-kid").expect("mesh signer")),
        bootstrap_iss: "https://bootstrap.test/".into(),
    }
}

struct RsaFixture {
    jwks: JwkSet,
    enc: EncodingKey,
    pem: String,
}

fn rsa_keypair(kid: &str) -> RsaFixture {
    let mut rng = rand::thread_rng();
    let private = RsaPrivateKey::new(&mut rng, 2048).expect("rsa");
    let pem = private
        .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
        .expect("pem")
        .to_string();
    let enc = EncodingKey::from_rsa_pem(pem.as_bytes()).expect("enc");
    let public = private.to_public_key();
    let n = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public.n().to_bytes_be());
    let e = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public.e().to_bytes_be());
    let jwk = json!({
        "keys": [{
            "kty": "RSA",
            "kid": kid,
            "use": "sig",
            "alg": "RS256",
            "n": n,
            "e": e
        }]
    });
    let jwks: JwkSet = serde_json::from_value(jwk).expect("jwks");
    RsaFixture { jwks, enc, pem }
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub fn mint_bootstrap_token(keys: &'static TestKeys, claims: Value) -> String {
    let mut hdr = Header::new(Algorithm::RS256);
    hdr.kid = Some("bootstrap-kid".into());
    encode(&hdr, &claims, &keys.bootstrap_enc).expect("bootstrap token")
}

pub fn sample_registry_record() -> AgentRegistryRecord {
    AgentRegistryRecord {
        agent_id: "acme/oncall-agent".into(),
        agent_version: "1.0.0".into(),
        agent_definition_digest: "sha256:abc".into(),
        owner_team: "platform".into(),
        allowed_workloads: vec!["spiffe://acme.local/ns/prod/sa/oncall-agent".into()],
        allowed_tools: vec!["tool:github::create_issue".into(), "tool:github::search_issues".into()],
        allowed_audiences: vec!["urn:trogon:mcp:backend:acme:github".into()],
        allowed_purposes: vec!["oncall-incident-triage".into()],
        mesh_token_ttl_s: None,
        metadata: None,
        lifecycle_state: "active".into(),
    }
}

pub fn build_service(
    keys: &'static TestKeys,
    record: AgentRegistryRecord,
) -> ExchangeService<InMemoryRegistry, RecordingAuditPublisher> {
    let jwks = JwksCache::new(keys.bootstrap_jwks.clone(), keys.mesh_jwks.clone());
    let trust = TrustBundleCache::from_pem("-----BEGIN TRUST BUNDLE-----".into());
    let registry = RegistryCache::new(InMemoryRegistry::new([record]));
    let audit = Arc::new(RecordingAuditPublisher::new());
    ExchangeService::new(
        DEFAULT_MESH_ISSUER.to_string(),
        keys.bootstrap_iss.clone(),
        jwks,
        trust,
        registry,
        keys.mesh_signer.clone(),
        audit,
    )
}

pub fn sample_exchange_request(subject_token: &str) -> StsExchangeRequest {
    StsExchangeRequest {
        subject_token: subject_token.to_string(),
        subject_token_type: "urn:ietf:params:oauth:token-type:jwt".into(),
        actor_token: "spiffe://acme.local/ns/prod/sa/oncall-agent".into(),
        audience: "urn:trogon:mcp:backend:acme:github".into(),
        scope: "tool:github::create_issue tool:github::search_issues".into(),
        purpose: "oncall-incident-triage".into(),
        requested_token_type: "urn:ietf:params:oauth:token-type:jwt".into(),
    }
}

pub fn bootstrap_claims(keys: &'static TestKeys) -> Value {
    json!({
        "sub": "user:alice",
        "iss": keys.bootstrap_iss,
        "aud": "trogon-mcp-gateway",
        "exp": now_secs() + 3600,
        "iat": now_secs(),
        "agent_id": "acme/oncall-agent",
        "agent_version": "1.0.0",
    })
}

pub fn decode_payload(token: &str) -> Value {
    use base64::Engine;
    let payload = token.split('.').nth(1).expect("jwt parts");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("b64");
    serde_json::from_slice(&bytes).expect("json")
}
