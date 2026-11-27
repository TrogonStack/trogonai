use std::sync::Arc;

use async_nats::{Client, SubscribeError};
use futures::StreamExt;
use tracing::warn;
use trogon_sandbox_contract::{LeaseId, ProviderKind};
use trogon_sandbox_protocol::subjects::provider_get_domain;
use trogon_sandbox_protocol::subscribe_verified;

use crate::{LeaseHolding, serve_domain};

/// Serves get_domain for every KubeVirt lease, one request at a time, until the client stops delivering them.
pub async fn run_get_domain_handler(client: Client, holder: Arc<dyn LeaseHolding>) -> Result<(), SubscribeError> {
    let subject = provider_get_domain(ProviderKind::KubeVirt, &LeaseId::new("*"));
    let mut requests = subscribe_verified(&client, subject).await?;
    while let Some(message) = requests.next().await {
        if let Err(error) = serve_domain(client.clone(), Arc::clone(&holder), message).await {
            warn!(%error, "failed to answer get_domain");
        }
    }
    Ok(())
}
