use std::sync::Arc;

use async_trait::async_trait;
use object_store::ObjectStore;
use object_store::aws::AmazonS3Builder;
use trogon_nats::RequestClient;
use trogon_sandbox_contract::{CredentialProvider, CredentialScope, TenantId};
use trogon_sandbox_credential_client::{CredentialClient, IssueParams};

use crate::constants::CREDENTIAL_TTL_SECONDS;
use crate::{VolumeSettings, VolumeStores, WorkspaceError};

/// Opens S3 stores with short-lived credentials the broker issues for each scope.
pub struct BrokeredVolumeStores<C> {
    client: C,
    settings: VolumeSettings,
}

impl<C> BrokeredVolumeStores<C> {
    pub fn new(client: C, settings: VolumeSettings) -> Self {
        Self { client, settings }
    }
}

#[async_trait]
impl<C> VolumeStores for BrokeredVolumeStores<C>
where
    C: RequestClient + Clone + Send + Sync + 'static,
{
    async fn open(&self, tenant: &TenantId, scope: &CredentialScope) -> Result<Arc<dyn ObjectStore>, WorkspaceError> {
        let (CredentialScope::S3ReadOnly { bucket, .. } | CredentialScope::S3ReadWrite { bucket, .. }) = scope else {
            return Err(WorkspaceError::ScopeUnsupported);
        };
        let issued = CredentialClient::new(self.client.clone(), CredentialProvider::S3, tenant.clone())
            .issue(IssueParams {
                pull_secret_id: self.settings.credential_id.clone(),
                scope: scope.clone(),
                ttl_seconds: CREDENTIAL_TTL_SECONDS,
                requester: None,
            })
            .await?;
        let mut parts = issued.token.expose_secret().splitn(3, ':');
        let (Some(access_key), Some(secret_key)) = (parts.next(), parts.next()) else {
            return Err(WorkspaceError::CredentialToken);
        };
        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(bucket)
            .with_region(self.settings.region.as_str())
            .with_access_key_id(access_key)
            .with_secret_access_key(secret_key);
        if let Some(session) = parts.next() {
            builder = builder.with_token(session);
        }
        if let Some(endpoint) = &self.settings.endpoint {
            builder = builder
                .with_endpoint(endpoint.as_str())
                .with_allow_http(endpoint.is_plain_http());
        }
        Ok(Arc::new(builder.build()?))
    }
}
