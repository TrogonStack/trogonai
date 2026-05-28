//! Latency baseline for the MCP gateway hot path (JWT verify, subject rewrite, authz decision).
//! These Criterion benches exist to catch regressions when we add CEL, schema cache, redaction,
//! and other ingress work—not to characterize peak throughput or size production capacity.
//! They run against in-memory fakes (no NATS, no SpiceDB) and should stay fast enough for local
//! runs and CI; treat results as a relative baseline, not a capacity-planning source of truth.

use std::collections::HashSet;
use std::hint::black_box;
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use criterion::{Criterion, criterion_group, criterion_main};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use tokio::runtime::Runtime;
use trogon_mcp_gateway::agent_identity::AgentIdentityMode;
use trogon_mcp_gateway::authz::{
    AllowAllPermissionChecker, AuthzContext, IdentitySource, PermissionChecker,
};
use trogon_mcp_gateway::jwt::{JwtIngressConfig, JwtMode, JwtValidator};
use trogon_mcp_gateway::subject::gateway_to_server_subject;

const HS_ISSUER: &str = "https://issuer.latency-baseline.test/";
const HS_AUD: &str = "trogon-mcp-gateway";
const HS_SECRET: &[u8] = b"latency-baseline-hs256-secret-32b-min!!";

static JWT_RUNTIME: OnceLock<Runtime> = OnceLock::new();
static JWT_VALIDATOR: OnceLock<Arc<JwtValidator>> = OnceLock::new();
static JWT_TOKEN: OnceLock<String> = OnceLock::new();

static AUTHZ_RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn hs256_jwt_config() -> JwtIngressConfig {
    let mut issuers = HashSet::new();
    issuers.insert(HS_ISSUER.into());
    JwtIngressConfig {
        mode: JwtMode::Validate,
        agent_identity_mode: AgentIdentityMode::Off,
        issuers: issuers.clone(),
        trusted_mint_issuers: issuers,
        audience: HS_AUD.into(),
        leeway_secs: 60,
        tenant_claim_key: "tenant".into(),
        bearer_header_name: "authorization".into(),
        hs256_secret: Some(HS_SECRET.to_vec()),
        rsa_public_key_pem: None,
        jwks_uri: None,
    }
}

fn exp_unix_secs_ahead(seconds: i64) -> serde_json::Value {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs() as i64;
    serde_json::Value::Number((now + seconds).into())
}

fn init_jwt_fixture() {
    JWT_RUNTIME.get_or_init(|| Runtime::new().expect("tokio runtime"));
    JWT_VALIDATOR.get_or_init(|| JwtValidator::try_new(hs256_jwt_config()).expect("jwt validator"));
    JWT_TOKEN.get_or_init(|| {
        let claims = serde_json::json!({
            "sub": "principal-latency-baseline",
            "iss": HS_ISSUER,
            "aud": HS_AUD,
            "tenant": "acme",
            "exp": exp_unix_secs_ahead(3_600),
        });
        let enc = EncodingKey::from_secret(HS_SECRET);
        let mut hdr = Header::new(Algorithm::HS256);
        hdr.typ = Some("JWT".into());
        encode(&hdr, &claims, &enc).expect("jwt encode")
    });
}

fn bench_jwt_verify(c: &mut Criterion) {
    init_jwt_fixture();
    let rt = JWT_RUNTIME.get().expect("jwt runtime");
    let validator = JWT_VALIDATOR.get().expect("jwt validator");
    let token = JWT_TOKEN.get().expect("jwt token");

    c.bench_function("bench_jwt_verify", |b| {
        b.iter(|| {
            let resolution = rt.block_on(validator.resolve(Some(token.as_str()), None, true));
            let _ = black_box(resolution);
        });
    });
}

fn bench_subject_parse(c: &mut Criterion) {
    const PREFIX: &str = "mcp";
    const INGRESS: &str = "mcp.gateway.request.filesystem.tools.call";

    c.bench_function("bench_subject_parse", |b| {
        b.iter(|| {
            let rewritten = gateway_to_server_subject(PREFIX, INGRESS);
            let _ = black_box(rewritten);
        });
    });
}

fn bench_no_op_decision(c: &mut Criterion) {
    let rt = AUTHZ_RUNTIME.get_or_init(|| Runtime::new().expect("tokio runtime"));
    let checker = AllowAllPermissionChecker;

    c.bench_function("bench_no_op_decision", |b| {
        b.iter(|| {
            let ctx = AuthzContext {
                tenant: Some("acme"),
                caller_sub: Some("principal-latency-baseline"),
                identity_source: IdentitySource::Jwt,
                server_id: "filesystem",
                jsonrpc_method: "tools/call",
                tool_name: Some("read_file"),
                resource_uri: None,
            };
            let allowed = rt.block_on(checker.authorize_mcp_request(ctx));
            let _ = black_box(allowed);
        });
    });
}

criterion_group!(benches, bench_jwt_verify, bench_subject_parse, bench_no_op_decision);
criterion_main!(benches);
