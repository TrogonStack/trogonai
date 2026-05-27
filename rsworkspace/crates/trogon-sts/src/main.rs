use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use futures::StreamExt;
use jsonwebtoken::jwk::JwkSet;
use tracing::{error, info, warn};

use trogon_sts::audit::NatsAuditPublisher;
use trogon_sts::cache::{HttpJwksSource, JwksCache, JwksSource, RegistryCache, TrustBundleCache};
use trogon_sts::chain_resolution::ChainResolutionMode;
use trogon_sts::circuit_breaker::CircuitBreaker;
use trogon_sts::exchange::{ExchangeService, default_bootstrap_issuer};
use trogon_sts::registry::{NatsRegistryClient, ResilientRegistry};
use trogon_sts::signer::{DynSigner, FileSigner};
use trogon_sts::spicedb::{NoOpSpiceDb, ResilientSpiceDb};
use trogon_sts::types::{StsExchangeRequest, StsTokenErrorResponse};
use trogon_sts::{
    DEFAULT_MESH_ISSUER, DEFAULT_QUEUE_GROUP, DEFAULT_REGISTRY_SUBJECT, EXCHANGE_SUBJECT, JWKS_KV_BUCKET,
    JWKS_KV_MESH_KEY,
};

type StsExchangeService = ExchangeService<
    ResilientRegistry<NatsRegistryClient<async_nats::Client>>,
    NatsAuditPublisher<async_nats::Client>,
    ResilientSpiceDb<NoOpSpiceDb>,
>;

#[derive(Debug, Parser)]
#[command(
    name = "trogon-sts",
    about = "NATS mesh Security Token Service (RFC 8693-style exchange)"
)]
struct Args {
    /// NATS server URL(s), comma-separated.
    #[arg(long, env = "NATS_URL")]
    nats_url: String,

    /// PEM file for mesh JWT signing (RS256).
    #[arg(long, env = "MCP_STS_SIGNING_KEY_PATH")]
    signing_key_pem: String,

    /// Mesh token `kid` header value (must match JWKS publisher).
    #[arg(long, default_value = "mesh-current", env = "MCP_STS_SIGNING_KID")]
    signing_kid: String,

    /// Mesh token issuer (`iss` claim on minted tokens).
    #[arg(long, default_value = DEFAULT_MESH_ISSUER, env = "MCP_STS_MESH_ISSUER")]
    mesh_issuer: String,

    /// HTTPS JWKS URL for bootstrap subject_token verification, or `kv://bucket/key`.
    #[arg(long, env = "MCP_STS_BOOTSTRAP_JWKS_URL")]
    bootstrap_issuer_jwks_url: String,

    /// PEM trust bundle for actor_token (SVID) verification.
    #[arg(long, env = "MCP_STS_TRUST_BUNDLE_PATH")]
    trust_bundle_path: String,

    /// Registry lookup subject.
    #[arg(long, default_value = DEFAULT_REGISTRY_SUBJECT, env = "MCP_STS_REGISTRY_SUBJECT")]
    registry_subject: String,

    /// Queue group for horizontal scale.
    #[arg(long, default_value = DEFAULT_QUEUE_GROUP, env = "MCP_STS_QUEUE_GROUP")]
    queue_group: String,
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
        warn!(error = %e, "mesh JWKS unavailable; using empty set for upstream mesh tokens");
        JwkSet { keys: vec![] }
    });

    let jwks_cache = JwksCache::new(bootstrap_jwks, mesh_jwks);
    spawn_jwks_watch(nats.clone(), jwks_cache.clone());

    let trust_pem = tokio::fs::read_to_string(&args.trust_bundle_path).await?;
    let trust_bundle = TrustBundleCache::from_pem(trust_pem);

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

    let audit = Arc::new(NatsAuditPublisher::new(nats.clone()));
    let service = Arc::new(ExchangeService::new(
        args.mesh_issuer.clone(),
        default_bootstrap_issuer(),
        jwks_cache,
        trust_bundle,
        registry,
        signer,
        audit,
        spicedb,
        chain_mode,
        require_purpose,
    ));

    let mut subscriber = nats
        .queue_subscribe(EXCHANGE_SUBJECT.to_string(), args.queue_group.clone())
        .await?;
    info!(
        subject = EXCHANGE_SUBJECT,
        queue_group = %args.queue_group,
        chain_resolution = ?chain_mode,
        "trogon-sts listening"
    );

    while let Some(msg) = subscriber.next().await {
        let service = Arc::clone(&service);
        let nats = nats.clone();
        tokio::spawn(async move {
            if let Some(reply) = msg.reply {
                handle_message(service, nats, msg.payload, reply).await;
            } else {
                warn!("exchange request missing reply subject");
            }
        });
    }

    Ok(())
}

async fn handle_message(service: Arc<StsExchangeService>, nats: async_nats::Client, payload: bytes::Bytes, reply: async_nats::Subject) {
    let request: StsExchangeRequest = match serde_json::from_slice(&payload) {
        Ok(r) => r,
        Err(e) => {
            let err = StsTokenErrorResponse {
                error: "invalid_request".into(),
                error_description: format!("json: {e}"),
            };
            let _ = nats
                .publish(reply, serde_json::to_vec(&err).unwrap_or_default().into())
                .await;
            return;
        }
    };

    let response = service.handle(request, None).await;
    let body = match response {
        Ok(ok) => serde_json::to_vec(&ok).unwrap_or_default(),
        Err(err) => serde_json::to_vec(&err).unwrap_or_default(),
    };
    if let Err(e) = nats.publish(reply, body.into()).await {
        error!(error = %e, "failed to publish STS reply");
    }
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
