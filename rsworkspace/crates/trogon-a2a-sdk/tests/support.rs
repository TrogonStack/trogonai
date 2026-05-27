//! Shared fixtures for live integration tests.

use std::sync::{Arc, LazyLock};

use base64::Engine;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::RsaPrivateKey;
use rsa::pkcs8::EncodePrivateKey;
use rsa::traits::PublicKeyParts;
use serde_json::{Value, json};
use trogon_agent_registry::{AgentRecord, LifecycleState};
use trogon_sts::DEFAULT_MESH_ISSUER;
use trogon_sts::attestor::{AttestationPolicy, build_attestor};
use trogon_sts::audit::RecordingAuditPublisher;
use trogon_sts::cache::{JwksCache, RegistryCache, TrustBundleCache};
use trogon_sts::chain_resolution::ChainResolutionMode;
use trogon_sts::exchange::ExchangeService;
use trogon_sts::registry::{AgentRegistryRecord, InMemoryRegistry};
use trogon_sts::signer::{DynSigner, FileSigner};
use trogon_sts::spicedb::NoOpSpiceDb;

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

pub fn bootstrap_claims(keys: &'static TestKeys, agent_id: &str) -> Value {
    json!({
        "sub": "user:alice",
        "iss": keys.bootstrap_iss,
        "aud": "trogon-mcp-gateway",
        "exp": now_secs() + 3600,
        "iat": now_secs(),
        "agent_id": agent_id,
        "agent_version": "1.0.0",
    })
}

pub fn decode_payload(token: &str) -> Value {
    let payload = token.split('.').nth(1).expect("jwt parts");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("b64");
    serde_json::from_slice(&bytes).expect("json")
}

pub const WKLA: &str = "spiffe://acme.local/ns/prod/sa/agent1";
pub const WKLB: &str = "spiffe://acme.local/ns/prod/sa/agent2";
pub const AGENT1: &str = "acme/agent1";
pub const AGENT2: &str = "acme/agent2";
pub const TEST_PURPOSE: &str = "integration-test";

pub fn agent1_registry_record() -> AgentRegistryRecord {
    AgentRegistryRecord {
        agent_id: AGENT1.into(),
        agent_version: "1.0.0".into(),
        agent_definition_digest: "sha256:agent1".into(),
        owner_team: "platform".into(),
        allowed_workloads: vec![WKLA.into()],
        allowed_tools: vec!["*".into()],
        allowed_audiences: vec![format!("urn:trogon:a2a:agent:acme:agent2")],
        allowed_purposes: vec![TEST_PURPOSE.into()],
        mesh_token_ttl_s: Some(120),
        metadata: None,
        lifecycle_state: "active".into(),
    }
}

pub fn agent2_registry_record() -> AgentRegistryRecord {
    AgentRegistryRecord {
        agent_id: AGENT2.into(),
        agent_version: "1.0.0".into(),
        agent_definition_digest: "sha256:agent2".into(),
        owner_team: "platform".into(),
        allowed_workloads: vec![WKLB.into()],
        allowed_tools: vec![],
        allowed_audiences: vec![format!("urn:trogon:a2a:agent:acme:agent2")],
        allowed_purposes: vec![TEST_PURPOSE.into()],
        mesh_token_ttl_s: Some(120),
        metadata: None,
        lifecycle_state: "active".into(),
    }
}

pub fn kv_agent_record(agent_id: &str, wkl: &str, audiences: Vec<String>) -> AgentRecord {
    AgentRecord {
        agent_id: agent_id.to_string(),
        agent_version: "1.0.0".to_string(),
        agent_definition_digest: "sha256:live".to_string(),
        owner_team: "platform".to_string(),
        allowed_workloads: vec![wkl.to_string()],
        allowed_tools: vec![],
        allowed_audiences: audiences,
        allowed_purposes: Some(vec![TEST_PURPOSE.to_string()]),
        mesh_token_ttl_s: Some(120),
        metadata: serde_json::Value::Null,
        lifecycle_state: LifecycleState::Active,
        created_at: "2026-05-27T00:00:00Z".to_string(),
        updated_at: "2026-05-27T00:00:00Z".to_string(),
    }
}

pub fn build_exchange_service(
    keys: &'static TestKeys,
) -> ExchangeService<InMemoryRegistry, RecordingAuditPublisher, NoOpSpiceDb> {
    let jwks = JwksCache::new(keys.bootstrap_jwks.clone(), keys.mesh_jwks.clone());
    let trust = TrustBundleCache::from_pem("-----BEGIN TRUST BUNDLE-----".into());
    let attestor = build_attestor(trust.clone(), AttestationPolicy::Shadow);
    let registry = RegistryCache::new(InMemoryRegistry::new([
        agent1_registry_record(),
        agent2_registry_record(),
    ]));
    let audit = Arc::new(RecordingAuditPublisher::new());
    ExchangeService::new(
        DEFAULT_MESH_ISSUER.to_string(),
        keys.bootstrap_iss.clone(),
        jwks,
        trust,
        attestor,
        AttestationPolicy::Shadow,
        registry,
        keys.mesh_signer.clone(),
        audit,
        NoOpSpiceDb,
        ChainResolutionMode::Cache,
        true,
    )
}

pub async fn spawn_nats() -> Option<(tokio::process::Child, String)> {
    use std::process::Stdio;
    use std::time::Duration;

    let port = 15000 + (std::process::id() as u16 % 20000);
    let url = format!("nats://127.0.0.1:{port}");
    let store_dir = tempfile::tempdir().ok()?;
    let config_dir = tempfile::tempdir().ok()?;
    let config_path = config_dir.path().join("nats.conf");
    let store_path = store_dir.path().to_string_lossy();
    let config = format!(
        r#"
port: {port}
jetstream {{
  store_dir: "{store_path}"
  strict: false
}}
"#
    );
    std::fs::write(&config_path, config).ok()?;
    let child = tokio::process::Command::new("nats-server")
        .arg("-c")
        .arg(&config_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    std::mem::forget(store_dir);
    std::mem::forget(config_dir);

    for _ in 0..50 {
        if async_nats::connect(&url).await.is_ok() {
            return Some((child, url));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}
