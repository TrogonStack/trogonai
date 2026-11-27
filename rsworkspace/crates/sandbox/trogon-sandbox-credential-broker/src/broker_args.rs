use std::future::Future;
use std::sync::Arc;

use async_nats::jetstream;
use clap::Parser;
use tracing::info;
use trogon_sandbox_observability::ObservabilityArgs;

use crate::{BrokerRunError, BrokerService, CredentialBroker, FenceMode, InProcessKey, ServiceSubscriptions};

/// Command line and environment configuration of the credential broker.
#[derive(Debug, Parser)]
#[command(
    name = "trogon-sandbox-credential-broker",
    about = "Issues short-lived scoped credential tokens over NATS"
)]
pub struct BrokerArgs {
    #[arg(long, env = "NATS_URL", default_value = "nats://127.0.0.1:4222")]
    pub nats_url: String,

    /// Keep reading legacy (no-AAD) credential records. Set to false once they have been migrated.
    #[arg(
        long,
        env = "SANDBOX_CREDENTIAL_ALLOW_LEGACY",
        default_value_t = true,
        action = clap::ArgAction::Set
    )]
    pub allow_legacy_credentials: bool,

    /// How a credential request without a policy fence is treated: `optional` accepts it (clients that predate
    /// the fence), `required` refuses it. A request that carries a fence is always verified against the
    /// committed policy record.
    #[arg(long, env = "SANDBOX_CREDENTIAL_FENCE_MODE", default_value = "optional")]
    pub fence_mode: FenceMode,

    /// Root key, the standard base64 encoding of 32 bytes.
    #[arg(long, env = "SANDBOX_KMS_KEY", hide_env_values = true)]
    pub kms_key: InProcessKey,

    #[command(flatten)]
    pub observability: ObservabilityArgs,
}

impl BrokerArgs {
    /// Connects to NATS, builds the broker and subscribes to the request subjects.
    pub async fn connect(&self) -> Result<ServiceSubscriptions, BrokerRunError> {
        let client = async_nats::connect(&self.nats_url).await?;
        let broker = CredentialBroker::jetstream(
            jetstream::new(client.clone()),
            Arc::new(self.kms_key.clone()),
            self.allow_legacy_credentials,
            self.fence_mode,
        )
        .await?;
        let health_client = client.clone();
        let subscriptions = Arc::new(BrokerService::new(Arc::new(broker))).subscribe(client).await?;
        self.observability.health().started_with_nats(&health_client);
        info!(nats_url = %self.nats_url, "credential broker subscribed");
        Ok(subscriptions)
    }

    /// Serves credential requests until `shutdown` completes.
    pub async fn serve(&self, shutdown: impl Future<Output = ()>) -> Result<(), BrokerRunError> {
        let health = self.observability.health().clone();
        self.connect()
            .await?
            .run(async move {
                shutdown.await;
                health.begin_shutdown();
            })
            .await;
        Ok(())
    }
}
