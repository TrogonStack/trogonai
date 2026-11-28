use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_auth::NatsKvApiKeyStore;
use trogon_sandbox_observability::WS_GATEWAY;
use trogon_sandbox_protocol::nats_connect::connect;
use trogon_std::env::SystemEnv;

use crate::{
    AppState, CapabilityRegistry, GatewayArgs, GatewayError, NatsKvSessionStore, NatsObservability, NatsSandboxBackend,
    ObservabilityConfig, SandboxBackend, parse_cors_origins, preview_from_env, router, serve_tls_with_shutdown,
    serve_with_shutdown,
};

/// Connects to NATS, starts observability and serves the gateway until `shutdown` resolves.
pub async fn run<F>(args: GatewayArgs, shutdown: F) -> Result<(), GatewayError>
where
    F: Future<Output = Result<(), GatewayError>> + Send + 'static,
{
    let (_tracing, _registry) =
        trogon_sandbox_observability::bootstrap("trogon-sandbox-ws-gateway", WS_GATEWAY, &args.observability)?;
    let session_ttl = Duration::from_secs(args.session_ttl_seconds);

    let nats = connect(&args.nats_url).await?;
    tracing::info!(url = %args.nats_url, "connected to NATS");
    let health = args.observability.health().clone();
    health.require_nats(&nats);

    let api_key_nats = match args.api_key_store_url.as_deref() {
        Some(url) if url != args.nats_url => connect(url).await?,
        _ => nats.clone(),
    };
    let api_keys = Arc::new(
        NatsKvApiKeyStore::new_with_bucket(async_nats::jetstream::new(api_key_nats), &args.api_key_bucket).await?,
    );
    let sessions = Arc::new(NatsKvSessionStore::new(async_nats::jetstream::new(nats.clone()), session_ttl).await?);

    let capabilities = CapabilityRegistry::new();
    let cancel = CancellationToken::new();
    CapabilityRegistry::spawn_heartbeat_watcher(nats.clone(), Arc::clone(&capabilities), cancel.clone());
    let observability = NatsObservability::new(
        nats.clone(),
        ObservabilityConfig {
            active_lease_bucket: args.active_lease_bucket,
        },
    );
    let backend: Arc<dyn SandboxBackend> = Arc::new(NatsSandboxBackend::new(nats, capabilities, observability));

    let preview_ttl = Duration::from_secs(args.preview_ttl_seconds);
    let preview = preview_from_env(&SystemEnv, args.preview_public_url.as_deref(), preview_ttl, session_ttl)?;
    let state = AppState {
        sessions,
        api_keys,
        backend,
        session_ttl,
        preview,
    };

    let listener = TcpListener::bind(args.listen).await.map_err(GatewayError::Bind)?;
    tracing::info!(listen = %args.listen, "sandbox ws gateway listening");
    let cors_origins = parse_cors_origins(&args.cors_origin)?;
    let app = router(state, cors_origins).merge(health.router());
    health.mark_started();
    let stopping = health.clone();
    let shutdown = async move {
        let outcome = shutdown.await;
        stopping.begin_shutdown();
        outcome
    };
    let served = match args.tls.files() {
        Some(tls) => serve_tls_with_shutdown(listener, &tls, app, shutdown).await,
        None => serve_with_shutdown(listener, app, shutdown).await,
    };
    cancel.cancel();
    served
}
