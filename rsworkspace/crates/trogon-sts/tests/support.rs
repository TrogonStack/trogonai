//! Shared test fixtures for STS exchange tests.

#![allow(dead_code)]

use std::sync::{Arc, LazyLock};
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use base64::Engine;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::RsaPrivateKey;
use rsa::pkcs8::EncodePrivateKey;
use rsa::traits::PublicKeyParts;
use serde_json::{Value, json};
use trogon_sts::DEFAULT_MESH_ISSUER;
use trogon_sts::attestor::{AttestationPolicy, build_attestor};
use trogon_sts::audit::RecordingAuditPublisher;
use trogon_sts::cache::{JwksCache, RegistryCache, TrustBundleCache};
use trogon_sts::chain_resolution::ChainResolutionMode;
use trogon_sts::error::StsError;
use trogon_sts::exchange::ExchangeService;
use trogon_sts::registry::{
    AgentRegistryRecord, InMemoryRegistry, RegistryLookup, RegistryLookupRequest, RegistryLookupResponse,
};
use trogon_sts::signer::{DynSigner, FileSigner};
use trogon_sts::spicedb::NoOpSpiceDb;
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
    build_service_with_audit(keys, record, ChainResolutionMode::Off).0
}

pub fn build_service_with_audit(
    keys: &'static TestKeys,
    record: AgentRegistryRecord,
    chain_mode: ChainResolutionMode,
) -> (
    ExchangeService<InMemoryRegistry, RecordingAuditPublisher>,
    Arc<RecordingAuditPublisher>,
) {
    let jwks = JwksCache::new(keys.bootstrap_jwks.clone(), keys.mesh_jwks.clone());
    let trust = TrustBundleCache::from_pem("-----BEGIN TRUST BUNDLE-----".into());
    let attestor = build_attestor(trust.clone(), AttestationPolicy::Shadow);
    let registry = RegistryCache::new(InMemoryRegistry::new([record]));
    let audit = Arc::new(RecordingAuditPublisher::new());
    let service = ExchangeService::new(
        DEFAULT_MESH_ISSUER.to_string(),
        keys.bootstrap_iss.clone(),
        jwks,
        trust,
        attestor,
        AttestationPolicy::Shadow,
        registry,
        keys.mesh_signer.clone(),
        Arc::clone(&audit),
        NoOpSpiceDb,
        chain_mode,
        true,
    );
    (service, audit)
}

pub fn build_service_with_mode(
    keys: &'static TestKeys,
    record: AgentRegistryRecord,
    chain_mode: ChainResolutionMode,
) -> ExchangeService<InMemoryRegistry, RecordingAuditPublisher> {
    build_service_with_mode_and_purpose(keys, record, chain_mode, true)
}

pub fn build_service_with_mode_and_purpose(
    keys: &'static TestKeys,
    record: AgentRegistryRecord,
    chain_mode: ChainResolutionMode,
    require_purpose: bool,
) -> ExchangeService<InMemoryRegistry, RecordingAuditPublisher> {
    let jwks = JwksCache::new(keys.bootstrap_jwks.clone(), keys.mesh_jwks.clone());
    let trust = TrustBundleCache::from_pem("-----BEGIN TRUST BUNDLE-----".into());
    let attestor = build_attestor(trust.clone(), AttestationPolicy::Shadow);
    let registry = RegistryCache::new(InMemoryRegistry::new([record]));
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
        chain_mode,
        require_purpose,
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
        "purpose": "oncall-incident-triage",
    })
}

pub fn test_spiffe_svid_pem(trust_domain: &str, path: &str) -> (String, String) {
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, SanType,
    };
    use rcgen::Ia5String;
    let spiffe_uri: Ia5String = format!("spiffe://{trust_domain}/{path}")
        .try_into()
        .expect("spiffe uri");
    let ca_key = KeyPair::generate().expect("ca key");
    let mut ca_params = CertificateParams::default();
    ca_params.distinguished_name.push(DnType::CommonName, "test-ca");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key).expect("ca");
    let ee_key = KeyPair::generate().expect("ee key");
    let mut ee_params = CertificateParams::default();
    ee_params.distinguished_name.push(DnType::CommonName, "workload");
    ee_params.subject_alt_names = vec![SanType::URI(spiffe_uri)];
    ee_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let ee = ee_params.signed_by(&ee_key, &ca, &ca_key).expect("ee");
    (ee.pem(), ca.pem())
}

pub async fn spawn_nats_js() -> Option<(tokio::process::Child, async_nats::Client)> {
    use std::process::Stdio;
    use std::time::Duration;

    let port = 16000 + (std::process::id() as u16 % 20000);
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
        if let Ok(client) = async_nats::connect(&url).await {
            return Some((child, client));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

pub fn decode_payload(token: &str) -> Value {
    use base64::Engine;
    let payload = token.split('.').nth(1).expect("jwt parts");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("b64");
    serde_json::from_slice(&bytes).expect("json")
}

#[derive(Clone)]
pub struct CountingRegistry {
    pub inner: InMemoryRegistry,
    lookups: Arc<AtomicUsize>,
}

impl CountingRegistry {
    pub fn new(records: impl IntoIterator<Item = AgentRegistryRecord>) -> Self {
        Self {
            inner: InMemoryRegistry::new(records),
            lookups: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn lookup_count(&self) -> usize {
        self.lookups.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl RegistryLookup for CountingRegistry {
    async fn lookup(&self, request: &RegistryLookupRequest) -> Result<AgentRegistryRecord, StsError> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        self.inner.lookup(request).await
    }

    async fn lookup_raw(&self, request: &RegistryLookupRequest) -> Result<RegistryLookupResponse, StsError> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        self.inner.lookup_raw(request).await
    }
}

pub fn build_counting_service(
    keys: &'static TestKeys,
    registry: CountingRegistry,
    chain_mode: ChainResolutionMode,
) -> (
    ExchangeService<CountingRegistry, RecordingAuditPublisher>,
    Arc<RecordingAuditPublisher>,
) {
    let jwks = JwksCache::new(keys.bootstrap_jwks.clone(), keys.mesh_jwks.clone());
    let trust = TrustBundleCache::from_pem("-----BEGIN TRUST BUNDLE-----".into());
    let attestor = build_attestor(trust.clone(), AttestationPolicy::Shadow);
    let registry = RegistryCache::new(registry);
    let audit = Arc::new(RecordingAuditPublisher::new());
    let service = ExchangeService::new(
        DEFAULT_MESH_ISSUER.to_string(),
        keys.bootstrap_iss.clone(),
        jwks,
        trust,
        attestor,
        AttestationPolicy::Shadow,
        registry,
        keys.mesh_signer.clone(),
        Arc::clone(&audit),
        NoOpSpiceDb,
        chain_mode,
        true,
    );
    (service, audit)
}
