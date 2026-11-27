//! The credentials and `Forward` identity a lease's egress proxy starts with.

use std::sync::Arc;

use trogon_sandbox_contract::{LeaseId, NetworkPolicy, TenantId};
use trogon_sandbox_credential_broker::CommittedFence;

use crate::config::FirecrackerConfig;
use crate::network::{BrokerConnector, CredentialRefresh, ForwardIdentity, LeaseCredentials};

/// What the lease's proxy needs beyond its policy. A worker that cannot inject credentials
/// refuses a policy with `Transform` rules at admission, so reaching here with one and no
/// broker is an error, never a lease that forwards uncredentialed.
///
/// When the worker can inject credentials, the lease gets a refreshable source even if its
/// policy names no provider yet, so a later policy update can add one. Each connection carries
/// the policy revision committed for the lease at that moment.
pub async fn for_lease(
    config: &FirecrackerConfig,
    committed: Option<&CommittedFence>,
    tenant: &TenantId,
    lease: &LeaseId,
    policy: &NetworkPolicy,
) -> anyhow::Result<(Option<LeaseCredentials>, Option<ForwardIdentity>)> {
    let credentials = match config.credential_broker_nats_url.as_deref() {
        Some(url) if config.injects_credentials() => {
            let mut credentials = LeaseCredentials::refreshable(CredentialRefresh {
                connector: Arc::new(BrokerConnector {
                    nats_url: url.to_string(),
                }),
                tenant: tenant.clone(),
                lease: lease.clone(),
                fence: committed.cloned(),
            });
            credentials.apply(policy).await?;
            Some(credentials)
        }
        _ if LeaseCredentials::providers_of(policy).is_empty() => None,
        _ => {
            anyhow::bail!("the network policy has Transform rules but this worker has no credential broker configured")
        }
    };
    let forward = config
        .oidc_signing_key_pem_file
        .clone()
        .filter(|_| config.forwards_requests())
        .map(|signing_key_pem_file| ForwardIdentity {
            signing_key_pem_file,
            tenant: tenant.clone(),
            lease: lease.clone(),
        });
    Ok((credentials, forward))
}
