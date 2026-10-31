use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::account_resolver::{AccountResolverError, RequestedAccount};
use crate::admission_keys::{AccountSigningKey, TenantAccount, TenantSigningKeys};
use crate::credentials::PresentedCredential;
#[allow(deprecated)]
use crate::credentials::api_key::ApiKeyVerifier;
use crate::credentials::mtls::MTlsVerifier;
use crate::credentials::oidc::OidcVerifier;
use crate::error::{AuthCalloutError, CredentialError};
use crate::jwt::{AccountName, MintedUserJwt, UserJwtSubject};
use crate::signing_key_source::MintingMaterial;
use crate::wire::ServerAuthRequestClaims;

/// Trait that the subscriber loop delegates auth decisions to.
#[async_trait::async_trait]
pub trait AuthDispatcher: Send + Sync + 'static {
    async fn dispatch(&self, request: ServerAuthRequestClaims) -> Result<MintedUserJwt, AuthCalloutError>;
}

pub struct CalloutDispatcherConfig {
    pub tenant_signing_keys: TenantSigningKeys,
    pub user_jwt_ttl: Duration,
    pub oidc: Option<Arc<dyn OidcVerifier>>,
    pub mtls: Option<Arc<dyn MTlsVerifier>>,
    #[allow(deprecated)]
    pub api_key: Option<Arc<dyn ApiKeyVerifier>>,
}

pub struct CalloutDispatcher {
    config: CalloutDispatcherConfig,
}

impl CalloutDispatcher {
    pub fn new(config: CalloutDispatcherConfig) -> Self {
        Self { config }
    }

    fn tenant(
        &self,
        requested: &RequestedAccount,
    ) -> Result<(TenantAccount, &AccountSigningKey), AccountResolverError> {
        let unknown = || AccountResolverError::Unknown(requested.as_str().to_owned());
        let tenant = TenantAccount::parse(requested.as_str()).map_err(|_| unknown())?;
        let signing_key = self
            .config
            .tenant_signing_keys
            .signing_key(&tenant)
            .ok_or_else(unknown)?;
        Ok((tenant, signing_key))
    }
}

#[async_trait::async_trait]
impl AuthDispatcher for CalloutDispatcher {
    async fn dispatch(&self, request: ServerAuthRequestClaims) -> Result<MintedUserJwt, AuthCalloutError> {
        let requested = request.requested_account()?;
        let (tenant, signing_key) = self.tenant(&requested)?;
        let account = AccountName::new(tenant.as_str());

        let claims = match request.presented_credential()? {
            PresentedCredential::BearerToken(token) => {
                let verifier = self
                    .config
                    .oidc
                    .as_ref()
                    .ok_or(CredentialError::VerifierUnavailable { scheme: "OIDC" })?;
                verifier.verify(&token, &account).await?
            }
            PresentedCredential::ClientCertificate(pem) => {
                let verifier = self
                    .config
                    .mtls
                    .as_ref()
                    .ok_or(CredentialError::VerifierUnavailable { scheme: "mTLS" })?;
                verifier.verify(&pem, &account).await?
            }
            PresentedCredential::ApiKey(key) => {
                #[allow(deprecated)]
                let verifier = self
                    .config
                    .api_key
                    .as_ref()
                    .ok_or(CredentialError::VerifierUnavailable { scheme: "API-key" })?;
                #[allow(deprecated)]
                verifier.verify(key.as_str(), &account).await?
            }
        };

        let user_nkey = request.user_nkey()?;
        let user_subject = UserJwtSubject::from_user_nkey(user_nkey);
        let material = MintingMaterial::on_behalf_of(signing_key, tenant);
        claims
            .mint(&material, &user_subject, SystemTime::now(), self.config.user_jwt_ttl)
            .map_err(AuthCalloutError::from)
    }
}

#[cfg(test)]
#[allow(deprecated)]
pub(crate) mod tests;
