use std::num::NonZeroU32;

use async_trait::async_trait;
use trogon_nats::RequestClient;
use trogon_sandbox_contract::{CredentialProvider, CredentialScope, PullSecretId, ShortLivedToken, TenantId};
use trogon_sandbox_credential_client::{CredentialClient, IssueParams};

use crate::{AcaError, CredentialIssuer};

/// Issues credentials by asking the credential broker over NATS.
#[derive(Debug, Clone)]
pub struct BrokerCredentialIssuer<C> {
    client: C,
}

impl<C> BrokerCredentialIssuer<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

#[async_trait]
impl<C> CredentialIssuer for BrokerCredentialIssuer<C>
where
    C: RequestClient + Clone + Send + Sync + 'static,
{
    async fn issue(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        pull_secret_id: &PullSecretId,
        scope: CredentialScope,
        ttl_seconds: NonZeroU32,
    ) -> Result<ShortLivedToken, AcaError> {
        let issued = CredentialClient::new(self.client.clone(), provider, tenant.clone())
            .issue(IssueParams {
                pull_secret_id: pull_secret_id.clone(),
                scope,
                ttl_seconds,
                requester: None,
            })
            .await?;
        Ok(issued.token)
    }
}
