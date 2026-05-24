use std::fmt;
use std::sync::Arc;

use a2a_nats::catalog::{
    CatalogRegistrarService, CatalogRegistrarServiceError, DiscoverService, DiscoverServiceError, KvCatalogStore,
    catalog_bucket_config,
};
use a2a_nats::{A2aPrefix, NatsConfig};
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_nats::jetstream::{
    JetStreamCreateKeyValue, JetStreamGetKeyValue, NatsJetStreamClient, is_create_key_value_already_exists,
};

use crate::config::{Args, ConfigError, config_from_args};

#[derive(Debug)]
pub enum ProvisionCatalogError {
    Create(async_nats::jetstream::context::CreateKeyValueError),
    Open(async_nats::jetstream::context::KeyValueError),
}

impl fmt::Display for ProvisionCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Create(error) => write!(f, "failed to create catalog KV bucket: {error}"),
            Self::Open(error) => write!(f, "failed to open existing catalog KV bucket: {error}"),
        }
    }
}

impl std::error::Error for ProvisionCatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Create(error) => Some(error),
            Self::Open(error) => Some(error),
        }
    }
}

#[derive(Debug)]
pub enum RuntimeError {
    Config(ConfigError),
    NatsConnect(trogon_nats::ConnectError),
    Provision(ProvisionCatalogError),
    Discover(DiscoverServiceError),
    Registrar(CatalogRegistrarServiceError),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(f, "{error}"),
            Self::NatsConnect(error) => write!(f, "NATS connection failed: {error}"),
            Self::Provision(error) => write!(f, "catalog bucket provisioning failed: {error}"),
            Self::Discover(error) => write!(f, "{error}"),
            Self::Registrar(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::NatsConnect(error) => Some(error),
            Self::Provision(error) => Some(error),
            Self::Discover(error) => Some(error),
            Self::Registrar(error) => Some(error),
        }
    }
}

impl From<ConfigError> for RuntimeError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<ProvisionCatalogError> for RuntimeError {
    fn from(error: ProvisionCatalogError) -> Self {
        Self::Provision(error)
    }
}

pub async fn run_with_args<E: trogon_std::env::ReadEnv>(args: Args, env: &E) -> Result<(), RuntimeError> {
    let (prefix, nats_config) = config_from_args(args, env)?;

    run_with_config(prefix, nats_config, env).await
}

pub async fn run_with_config<E: trogon_std::env::ReadEnv>(
    prefix: A2aPrefix,
    nats_config: NatsConfig,
    env: &E,
) -> Result<(), RuntimeError> {
    let connect_timeout = a2a_nats::nats_connect_timeout(env);
    let nats_client = trogon_nats::connect(&nats_config, connect_timeout)
        .await
        .map_err(RuntimeError::NatsConnect)?;

    let js_context = async_nats::jetstream::new(nats_client.clone());
    let js_client = NatsJetStreamClient::new(js_context);

    let kv_store = provision_catalog_bucket(&js_client).await?;
    let catalog = KvCatalogStore::new(kv_store);
    let import_gate = Arc::new(a2a_nats::catalog::resolve_import_gate(env).await);
    if import_gate.is_configured() {
        info!("SpiceDB federated import gate configured for cross-Account discover imports");
    } else {
        info!("SpiceDB federated import gate deny-only (set A2A_SPICEDB_ENDPOINT + A2A_SPICEDB_TOKEN to enable)");
    }
    let _import_gate = import_gate;

    let shutdown = CancellationToken::new();
    let shutdown_for_task = shutdown.clone();

    tokio::spawn(async move {
        trogon_std::signal::shutdown_signal().await;
        shutdown_for_task.cancel();
    });

    let (discover_res, registrar_res) = tokio::join!(
        DiscoverService::new(prefix.clone(), catalog.clone(), nats_client.clone()).run(shutdown.clone()),
        CatalogRegistrarService::new(prefix.clone(), catalog, nats_client).run(shutdown.clone()),
    );
    discover_res.map_err(RuntimeError::Discover)?;
    registrar_res.map_err(RuntimeError::Registrar)?;

    info!(prefix = %prefix, "A2A discover + catalog registrar shutdown complete");
    Ok(())
}

async fn provision_catalog_bucket(
    js: &NatsJetStreamClient,
) -> Result<async_nats::jetstream::kv::Store, ProvisionCatalogError> {
    let config = catalog_bucket_config();
    let bucket = config.bucket.clone();

    let store = match js.create_key_value(config).await {
        Ok(store) => store,
        Err(source) if is_create_key_value_already_exists(&source) => js
            .get_key_value(bucket.clone())
            .await
            .map_err(ProvisionCatalogError::Open)?,
        Err(source) => return Err(ProvisionCatalogError::Create(source)),
    };

    info!(bucket = %bucket, "Provisioned A2A agent catalog KV bucket");
    Ok(store)
}
