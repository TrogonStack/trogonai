//! What a lease's egress proxy uses to act for the lease: the credentials its `Transform` rules
//! inject and the identity its `Forward` rules sign.
//!
//! Both live on the host. The guest only ever sees the proxy, so neither a credential value nor
//! the OIDC signing key reaches it.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use tokio::sync::RwLock;
use trogon_sandbox_contract::{
    CredentialProvider, CredentialRequest, DomainRuleAction, LeaseId, NetworkPolicy, PolicyFence, SandboxId,
    ShortLivedToken, TenantId,
};
use trogon_sandbox_credential_broker::CommittedFence;
use trogon_sandbox_egress_proxy::credential_source::{
    BrokerCredentialSource, CredentialSource, CredentialSourceError, UnconfiguredCredentialSource,
};
use trogon_sandbox_egress_proxy::oidc_signer::OidcSigner;
use trogon_sandbox_egress_proxy::proxy::OidcIdentityContext;
use trogon_sandbox_egress_proxy::server::OIDC_ISSUER;

/// Whether `policy` forwards requests to another service under the lease's OIDC identity.
pub fn forwards_requests(policy: &NetworkPolicy) -> bool {
    matches!(policy, NetworkPolicy::UserDefined { rules, .. }
        if rules.iter().any(|rule| matches!(rule.action, DomainRuleAction::Forward(_))))
}

/// The identity a lease's `Forward` rules sign requests with. The key is read when the proxy
/// starts, so a lease never holds it longer than the proxy needs.
#[derive(Clone, Debug)]
pub struct ForwardIdentity {
    pub signing_key_pem_file: PathBuf,
    pub tenant: TenantId,
    pub lease: LeaseId,
}

impl ForwardIdentity {
    pub fn context(&self) -> anyhow::Result<OidcIdentityContext> {
        let pem = std::fs::read_to_string(&self.signing_key_pem_file).context("reading the OIDC signing key")?;
        Ok(OidcIdentityContext {
            signer: OidcSigner::from_ec_pkcs8_pem(&pem).context("loading the OIDC signing key")?,
            issuer: OIDC_ISSUER.to_string(),
            subject: SandboxId::new(self.lease.as_str()).context("naming the lease as an OIDC subject")?,
            tenant_id: self.tenant.clone(),
            lease_id: self.lease.clone(),
        })
    }
}

/// A credential source the proxy keeps while the lease replaces what is behind it.
struct SwappableSource {
    current: RwLock<Arc<dyn CredentialSource>>,
}

#[async_trait]
impl CredentialSource for SwappableSource {
    async fn fetch(&self, request: &CredentialRequest) -> Result<ShortLivedToken, CredentialSourceError> {
        let current = Arc::clone(&*self.current.read().await);
        current.fetch(request).await
    }
}

/// Connects a credential source for a set of providers.
#[async_trait]
pub trait CredentialConnector: Send + Sync {
    async fn connect(
        &self,
        tenant: &TenantId,
        providers: &[CredentialProvider],
        fence: Option<&PolicyFence>,
    ) -> anyhow::Result<Arc<dyn CredentialSource>>;
}

/// Connects to the credential broker over NATS.
pub struct BrokerConnector {
    pub nats_url: String,
}

#[async_trait]
impl CredentialConnector for BrokerConnector {
    async fn connect(
        &self,
        tenant: &TenantId,
        providers: &[CredentialProvider],
        fence: Option<&PolicyFence>,
    ) -> anyhow::Result<Arc<dyn CredentialSource>> {
        let source = match fence {
            Some(fence) => {
                BrokerCredentialSource::connect_fenced(&self.nats_url, tenant.clone(), providers, fence.clone()).await
            }
            None => BrokerCredentialSource::connect(&self.nats_url, tenant.clone(), providers).await,
        }
        .map_err(|error| {
            anyhow::anyhow!("credential injection was requested but the credential broker is unreachable: {error}")
        })?;
        Ok(Arc::new(source))
    }
}

/// How a running lease reconnects its credential source when its policy changes: to reach a
/// provider the policy did not name before, and to carry the policy revision the control plane
/// committed for the update.
pub struct CredentialRefresh {
    pub connector: Arc<dyn CredentialConnector>,
    pub tenant: TenantId,
    pub lease: LeaseId,
    pub fence: Option<CommittedFence>,
}

/// Where a lease's egress proxy gets the credentials its `Transform` rules inject. Real values
/// are fetched by the proxy per request and never reach the guest.
#[derive(Clone)]
pub struct LeaseCredentials {
    pub(crate) source: Arc<dyn CredentialSource>,
    providers: Vec<CredentialProvider>,
    swap: Option<Arc<SwappableSource>>,
    refresh: Option<Arc<CredentialRefresh>>,
}

impl LeaseCredentials {
    /// A fixed source for `providers`. A policy update cannot add a provider to it.
    pub fn new(source: Arc<dyn CredentialSource>, providers: Vec<CredentialProvider>) -> Self {
        Self {
            source,
            providers,
            swap: None,
            refresh: None,
        }
    }

    /// A source with no providers yet. [`Self::apply`] connects the ones a policy names, now
    /// and on every later update.
    pub fn refreshable(refresh: CredentialRefresh) -> Self {
        let swap = Arc::new(SwappableSource {
            current: RwLock::new(Arc::new(UnconfiguredCredentialSource)),
        });
        Self {
            source: swap.clone(),
            providers: Vec::new(),
            swap: Some(swap),
            refresh: Some(Arc::new(refresh)),
        }
    }

    /// Every provider a `Transform` rule of `policy` names, once each.
    pub fn providers_of(policy: &NetworkPolicy) -> Vec<CredentialProvider> {
        let NetworkPolicy::UserDefined { rules, .. } = policy else {
            return Vec::new();
        };
        let mut providers = Vec::new();
        for rule in rules {
            if let DomainRuleAction::Transform(transform) = &rule.action
                && !providers.contains(&transform.credential.provider)
            {
                providers.push(transform.credential.provider);
            }
        }
        providers
    }

    /// Makes the source serve `policy`. A refreshable source reconnects for every provider the
    /// lease has needed so far plus the policy's, at the revision committed now; a fixed source
    /// can only serve the providers it was given.
    pub async fn apply(&mut self, policy: &NetworkPolicy) -> anyhow::Result<()> {
        let mut providers = self.providers.clone();
        for provider in Self::providers_of(policy) {
            if !providers.contains(&provider) {
                providers.push(provider);
            }
        }
        let (Some(refresh), Some(swap)) = (&self.refresh, &self.swap) else {
            if let Some(missing) = providers.iter().find(|p| !self.providers.contains(p)) {
                anyhow::bail!(
                    "the lease's proxy was started without a credential source for provider {}; it cannot be added while the lease runs",
                    missing.as_str()
                );
            }
            return Ok(());
        };
        if providers.is_empty() {
            return Ok(());
        }
        let fence = match &refresh.fence {
            Some(committed) => committed.resolve_lease(&refresh.tenant, &refresh.lease).await?,
            None => None,
        };
        let source = refresh
            .connector
            .connect(&refresh.tenant, &providers, fence.as_ref())
            .await?;
        *swap.current.write().await = source;
        self.providers = providers;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
