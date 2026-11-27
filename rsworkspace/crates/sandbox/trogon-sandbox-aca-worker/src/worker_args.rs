use std::num::NonZeroU32;
use std::path::PathBuf;

use clap::Parser;
use trogon_sandbox_contract::{OciImageRef, PullSecretId};
use trogon_sandbox_observability::ObservabilityArgs;
use trogon_sandbox_volumes::StoreResourceName;
use url::Url;

use crate::ProviderConfig;
use crate::constants::{DEFAULT_LOGIN_ENDPOINT, MAX_LEASE_LIFETIME_SECONDS};

/// Command line and environment configuration of the worker.
#[derive(Parser, Debug)]
#[command(
    version,
    about = "Sandbox Azure Container Apps worker: runs each lease in a Dynamic Sessions session"
)]
pub struct WorkerArgs {
    /// Journal for the tenant and attempt each lease belongs to, so the dispatch fence survives a
    /// restart. Unset keeps it in memory.
    #[arg(long, env = "SANDBOX_DISPATCH_FENCE_FILE")]
    pub dispatch_fence_file: Option<PathBuf>,

    /// NATS server the worker serves leases from.
    #[arg(long, env = "SANDBOX_NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,

    /// The session pool management endpoint the data plane calls go to.
    #[arg(long, env = "SANDBOX_ACA_POOL_MANAGEMENT_ENDPOINT")]
    pub pool_management_endpoint: Url,

    /// The login authority service principal tokens are requested from.
    #[arg(long, env = "SANDBOX_ACA_LOGIN_ENDPOINT", default_value = DEFAULT_LOGIN_ENDPOINT)]
    pub login_endpoint: Url,

    /// The name of the session pool.
    #[arg(long, env = "SANDBOX_ACA_SESSION_POOL_NAME")]
    pub session_pool_name: String,

    /// The image a lease boots when its own reference names none.
    #[arg(long, env = "SANDBOX_ACA_DEFAULT_IMAGE")]
    pub default_image: Option<String>,

    /// The broker secret that holds the service principal.
    #[arg(long, env = "SANDBOX_ACA_AZURE_CREDENTIAL_ID", default_value = "azure-sp")]
    pub azure_credential_id: String,

    /// The broker secret that holds the volume storage credential.
    #[arg(long, env = "SANDBOX_ACA_VOLUME_CREDENTIAL_ID", default_value = "azure-volume")]
    pub volume_credential_id: String,

    /// The storage account volumes live in. Without it S3 volume workspaces are refused.
    #[arg(long, env = "SANDBOX_ACA_VOLUME_STORAGE_ACCOUNT")]
    pub volume_storage_account: Option<StoreResourceName>,

    /// The longest a lease may live, in seconds.
    #[arg(long, env = "SANDBOX_MAX_LEASE_LIFETIME_SECONDS", default_value_t = MAX_LEASE_LIFETIME_SECONDS)]
    pub max_lease_lifetime_seconds: NonZeroU32,

    /// How many leases the worker advertises it can hold at once.
    #[arg(long, env = "SANDBOX_MAX_CONCURRENT_LEASES", default_value_t = 4)]
    pub max_concurrent_leases: u32,

    /// The Azure region the worker advertises, when it should be placeable by region.
    #[arg(long, env = "SANDBOX_ACA_REGION")]
    pub region: Option<String>,

    #[command(flatten)]
    pub observability: ObservabilityArgs,
}

impl WorkerArgs {
    /// The Azure side of the configuration.
    pub fn provider_config(&self) -> ProviderConfig {
        ProviderConfig {
            management_endpoint: self.pool_management_endpoint.clone(),
            login_endpoint: self.login_endpoint.clone(),
            session_pool_name: self.session_pool_name.clone(),
            default_image: self.default_image.as_deref().map(OciImageRef::parse_unqualified),
            azure_credential_id: PullSecretId::new(self.azure_credential_id.clone()),
            volume_credential_id: PullSecretId::new(self.volume_credential_id.clone()),
            volume_storage_account: self.volume_storage_account.clone(),
        }
    }
}
