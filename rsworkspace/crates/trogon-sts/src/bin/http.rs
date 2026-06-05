use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::serve;
use clap::Parser;
use futures::StreamExt;
use jsonwebtoken::jwk::JwkSet;
use tracing::{info, warn};

use trogon_sts::attestor::{AttestationPolicy, build_attestor};
use trogon_sts::audit::NatsAuditPublisher;
use trogon_sts::cache::{
    HttpJwksSource, JwksCache, JwksSource, RegistryCache, TrustBundleCache, load_trust_bundle_from_kv,
    spawn_trust_bundle_watch,
};
use trogon_sts::chain_resolution::ChainResolutionMode;
use trogon_sts::circuit_breaker::CircuitBreaker;
use trogon_sts::exchange::{ExchangeService, default_bootstrap_issuer};
use trogon_sts::http::{StsHttpState, router};
use trogon_sts::registry::{NatsRegistryClient, ResilientRegistry};
use trogon_sts::signer::{DynSigner, FileSigner};
use trogon_sts::spicedb::{NoOpSpiceDb, ResilientSpiceDb};
use trogon_sts::{
    DEFAULT_MESH_ISSUER, DEFAULT_REGISTRY_SUBJECT, JWKS_KV_BUCKET, JWKS_KV_MESH_KEY, TRUST_BUNDLES_KV_BUCKET,
};

#[derive(Debug, Parser)]
#[command(
    name = "trogon-sts-http",
    about = "HTTP front for the Security Token Service (RFC 8693)"
)]
struct Args {
    #[arg(long, env = "NATS_URL")]
    nats_url: String,

    #[arg(long, env = "MCP_STS_SIGNING_KEY_PATH")]
    signing_key_pem: String,

    #[arg(long, default_value = "mesh-current", env = "MCP_STS_SIGNING_KID")]
    signing_kid: String,

    #[arg(long, default_value = DEFAULT_MESH_ISSUER, env = "MCP_STS_MESH_ISSUER")]
    mesh_issuer: String,

    #[arg(long, env = "MCP_STS_BOOTSTRAP_JWKS_URL")]
    bootstrap_issuer_jwks_url: String,

    #[arg(long, env = "MCP_STS_TRUST_BUNDLE_PATH")]
    trust_bundle_path: String,

    #[arg(long, default_value = "local", env = "MCP_STS_TRUST_DOMAIN")]
    trust_domain: String,

    #[arg(long, default_value = DEFAULT_REGISTRY_SUBJECT, env = "MCP_STS_REGISTRY_SUBJECT")]
    registry_subject: String,

    #[arg(long, default_value = "mcp", env = "MCP_STS_PREFIX")]
    mcp_prefix: String,

    #[arg(long, default_value = "0.0.0.0:8081", env = "MCP_STS_HTTP_LISTEN")]
    listen: SocketAddr,

    #[arg(long, env = "MCP_STS_PUBLIC_URL")]
    public_url: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    run(args).await
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let nats = async_nats::ConnectOptions::new().connect(&args.nats_url).await?;
    info!(url = %args.nats_url, "connected to NATS");

    let bootstrap_jwks = load_bootstrap_jwks(&args.bootstrap_issuer_jwks_url, &nats).await?;
    let mesh_jwks = load_mesh_jwks(&nats).await.unwrap_or_else(|e| {
        warn!(error = %e, "mesh JWKS unavailable; using empty set");
        JwkSet { keys: vec![] }
    });

    let jwks_cache = JwksCache::new(bootstrap_jwks, mesh_jwks);
    spawn_jwks_watch(nats.clone(), jwks_cache.clone());

    let trust_pem = tokio::fs::read_to_string(&args.trust_bundle_path).await?;
    let trust_bundle = TrustBundleCache::from_pem_for_domain(&args.trust_domain, trust_pem);
    spawn_trust_bundle_watch(nats.clone(), trust_bundle.clone(), TRUST_BUNDLES_KV_BUCKET);
    if let Ok((domain, pem)) = load_trust_bundle_from_kv(&nats, TRUST_BUNDLES_KV_BUCKET, &args.trust_domain).await {
        trust_bundle.upsert_domain(domain, pem).await;
    }

    let attestation_policy = AttestationPolicy::from_env();
    let attestor = build_attestor(trust_bundle.clone(), attestation_policy);

    let signing_pem = tokio::fs::read_to_string(&args.signing_key_pem).await?;
    let signer: DynSigner = Arc::new(FileSigner::from_rsa_pem(&signing_pem, &args.signing_kid)?);

    let registry_client = ResilientRegistry::new(
        NatsRegistryClient::new(nats.clone(), args.registry_subject.clone()),
        CircuitBreaker::default(),
    );
    let registry = RegistryCache::new(registry_client);
    let spicedb = ResilientSpiceDb::new(NoOpSpiceDb, CircuitBreaker::default());
    let chain_mode = ChainResolutionMode::from_env();
    let require_purpose = trogon_sts::exchange::require_purpose_from_env();

    let audit = Arc::new(NatsAuditPublisher::new(nats.clone(), args.mcp_prefix.as_str()));
    let service = Arc::new(ExchangeService::new(
        args.mesh_issuer.clone(),
        default_bootstrap_issuer(),
        jwks_cache.clone(),
        trust_bundle,
        attestor,
        attestation_policy,
        registry,
        signer,
        audit,
        spicedb,
        chain_mode,
        require_purpose,
    ));

    let state = StsHttpState {
        service,
        jwks: jwks_cache,
        issuer: args.mesh_issuer.clone(),
        external_base_url: args.public_url.clone(),
    };
    let app = router(state).into_make_service_with_connect_info::<SocketAddr>();

    info!(listen = %args.listen, "trogon-sts-http listening");
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    serve(listener, app).await?;
    Ok(())
}

async fn load_bootstrap_jwks(url: &str, nats: &async_nats::Client) -> Result<JwkSet, String> {
    if let Some(path) = url.strip_prefix("kv://") {
        let (bucket, key) = path
            .split_once('/')
            .ok_or_else(|| "kv url must be kv://bucket/key".to_string())?;
        return read_jwks_from_kv(nats, bucket, key).await;
    }
    let source = HttpJwksSource::new(url.to_string()).map_err(|e| e.to_string())?;
    source.fetch_bootstrap_jwks().await.map_err(|e| e.to_string())
}

async fn load_mesh_jwks(nats: &async_nats::Client) -> Result<JwkSet, String> {
    read_jwks_from_kv(nats, JWKS_KV_BUCKET, JWKS_KV_MESH_KEY).await
}

async fn read_jwks_from_kv(nats: &async_nats::Client, bucket: &str, key: &str) -> Result<JwkSet, String> {
    let js = async_nats::jetstream::new(nats.clone());
    let store = js.get_key_value(bucket).await.map_err(|e| e.to_string())?;
    let value = store
        .get(key)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("kv key {key} missing"))?;
    let body = String::from_utf8(value.to_vec()).map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| e.to_string())
}

fn spawn_jwks_watch(nats: async_nats::Client, cache: JwksCache) {
    tokio::spawn(async move {
        loop {
            match watch_mesh_jwks(&nats, cache.clone()).await {
                Ok(()) => warn!("mesh jwks watch ended; restarting"),
                Err(e) => {
                    warn!(error = %e, "mesh jwks watch failed; retrying");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });
}

async fn watch_mesh_jwks(nats: &async_nats::Client, cache: JwksCache) -> Result<(), String> {
    let js = async_nats::jetstream::new(nats.clone());
    let store = js.get_key_value(JWKS_KV_BUCKET).await.map_err(|e| e.to_string())?;
    let mut watcher = store.watch(JWKS_KV_MESH_KEY).await.map_err(|e| e.to_string())?;
    while let Some(entry) = watcher.next().await {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.operation != async_nats::jetstream::kv::Operation::Delete
            && let Ok(set) = serde_json::from_slice::<JwkSet>(&entry.value)
        {
            cache.update_mesh(set).await;
        }
    }
    Ok(())
}
