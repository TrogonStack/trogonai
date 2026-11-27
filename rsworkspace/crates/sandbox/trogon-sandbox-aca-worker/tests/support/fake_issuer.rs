use std::num::NonZeroU32;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use trogon_sandbox_aca_worker::{AcaError, CredentialIssuer};
use trogon_sandbox_contract::{CredentialProvider, CredentialScope, PullSecretId, ShortLivedToken, TenantId};

/// Answers credential requests with fixed tokens and records what was asked for.
pub struct FakeIssuer {
    azure_token: String,
    volume_token: String,
    failing: Mutex<Option<CredentialProvider>>,
    scopes: Mutex<Vec<CredentialScope>>,
    ttls: Mutex<Vec<u32>>,
}

impl FakeIssuer {
    pub fn new() -> Self {
        Self::with_tokens("client:secret:tenant", "account:key")
    }

    pub fn with_tokens(azure_token: &str, volume_token: &str) -> Self {
        Self {
            azure_token: azure_token.to_owned(),
            volume_token: volume_token.to_owned(),
            failing: Mutex::default(),
            scopes: Mutex::default(),
            ttls: Mutex::default(),
        }
    }

    pub fn fail(&self, provider: CredentialProvider) {
        *self.failing.lock().unwrap_or_else(PoisonError::into_inner) = Some(provider);
    }

    pub fn recover(&self) {
        *self.failing.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    pub fn scopes(&self) -> Vec<CredentialScope> {
        self.scopes.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn ttls(&self) -> Vec<u32> {
        self.ttls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl CredentialIssuer for FakeIssuer {
    async fn issue(
        &self,
        provider: CredentialProvider,
        _tenant: &TenantId,
        _pull_secret_id: &PullSecretId,
        scope: CredentialScope,
        ttl_seconds: NonZeroU32,
    ) -> Result<ShortLivedToken, AcaError> {
        self.scopes.lock().unwrap_or_else(PoisonError::into_inner).push(scope);
        self.ttls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(ttl_seconds.get());
        if *self.failing.lock().unwrap_or_else(PoisonError::into_inner) == Some(provider) {
            return Err(AcaError::Auth("broker is down".to_owned()));
        }
        Ok(ShortLivedToken::new(match provider {
            CredentialProvider::Azure => self.azure_token.as_str(),
            _ => self.volume_token.as_str(),
        }))
    }
}
