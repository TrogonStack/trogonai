use std::num::NonZeroU32;

use async_trait::async_trait;
use trogon_sandbox_contract::{CredentialProvider, CredentialScope, PullSecretId, ShortLivedToken, TenantId};

use crate::AcaError;

/// Where the worker gets the short-lived credentials it needs from.
#[async_trait]
pub trait CredentialIssuer: Send + Sync {
    async fn issue(
        &self,
        provider: CredentialProvider,
        tenant: &TenantId,
        pull_secret_id: &PullSecretId,
        scope: CredentialScope,
        ttl_seconds: NonZeroU32,
    ) -> Result<ShortLivedToken, AcaError>;
}
