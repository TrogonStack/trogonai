//! Where the worker and the lease proxy get real credential values from.

use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use trogon_sandbox_contract::{CredentialProvider, CredentialRequest, PolicyFence, ShortLivedToken, TenantId};
use trogon_sandbox_egress_proxy::credential_source::{BrokerCredentialSource, CredentialSource, CredentialSourceError};

/// Where the worker gets real credential values from, and where the lease
/// proxy gets them for `Transform` rules.
#[async_trait]
pub trait CredentialSources: Send + Sync {
    async fn source_for(
        &self,
        tenant: &TenantId,
        providers: &[CredentialProvider],
    ) -> anyhow::Result<Arc<dyn CredentialSource>>;

    /// [`Self::source_for`] with every request stamped with `fence`, so the
    /// broker resolves only references the lease's committed policy names.
    /// Sources that do not talk to the broker ignore the fence.
    async fn fenced_source_for(
        &self,
        tenant: &TenantId,
        providers: &[CredentialProvider],
        _fence: &PolicyFence,
    ) -> anyhow::Result<Arc<dyn CredentialSource>> {
        self.source_for(tenant, providers).await
    }

    fn broker_nats_url(&self) -> Option<&str>;
}

pub struct NatsCredentialSources {
    nats_url: String,
}

impl NatsCredentialSources {
    pub fn new(nats_url: impl Into<String>) -> Self {
        Self {
            nats_url: nats_url.into(),
        }
    }
}

#[async_trait]
impl CredentialSources for NatsCredentialSources {
    async fn source_for(
        &self,
        tenant: &TenantId,
        providers: &[CredentialProvider],
    ) -> anyhow::Result<Arc<dyn CredentialSource>> {
        let source = BrokerCredentialSource::connect(&self.nats_url, tenant.clone(), providers)
            .await
            .context("connecting to the credential broker")?;
        Ok(Arc::new(source))
    }

    async fn fenced_source_for(
        &self,
        tenant: &TenantId,
        providers: &[CredentialProvider],
        fence: &PolicyFence,
    ) -> anyhow::Result<Arc<dyn CredentialSource>> {
        let source = BrokerCredentialSource::connect_fenced(&self.nats_url, tenant.clone(), providers, fence.clone())
            .await
            .context("connecting to the credential broker")?;
        Ok(Arc::new(source))
    }

    fn broker_nats_url(&self) -> Option<&str> {
        Some(&self.nats_url)
    }
}

/// Serves the same value for every request. For local runs and tests only.
pub struct FixedCredentialSources {
    value: String,
}

impl FixedCredentialSources {
    pub fn new(value: impl Into<String>) -> Self {
        Self { value: value.into() }
    }
}

pub(crate) struct FixedSource(pub(crate) String);

#[async_trait]
impl CredentialSource for FixedSource {
    async fn fetch(&self, _request: &CredentialRequest) -> Result<ShortLivedToken, CredentialSourceError> {
        Ok(ShortLivedToken::new(self.0.clone()))
    }
}

#[async_trait]
impl CredentialSources for FixedCredentialSources {
    async fn source_for(
        &self,
        _tenant: &TenantId,
        _providers: &[CredentialProvider],
    ) -> anyhow::Result<Arc<dyn CredentialSource>> {
        Ok(Arc::new(FixedSource(self.value.clone())))
    }

    fn broker_nats_url(&self) -> Option<&str> {
        None
    }
}
