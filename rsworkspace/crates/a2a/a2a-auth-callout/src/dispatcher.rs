use std::sync::Arc;
use std::time::Duration;

use a2a_nats::A2aPrefix;
use trogon_std::{EpochClock, SystemClock};

use crate::account_resolver::{AccountResolverError, RequestedAccount};
use crate::admission_keys::{AccountSigningKey, TenantAccount, TenantSigningKeys};
use crate::credentials::PresentedCredential;
#[allow(deprecated)]
use crate::credentials::api_key::ApiKeyVerifier;
use crate::credentials::mtls::MTlsVerifier;
use crate::credentials::oidc::OidcVerifier;
use crate::error::{AuthCalloutError, CredentialError};
use crate::jwt::{AccountName, MintedUserJwt, UserJwtClaims, UserJwtSubject};
use crate::permissions::CallerGrant;
use crate::signing_key_source::MintingMaterial;
use crate::wire::ServerAuthRequestClaims;
use crate::{CredentialExpiry, VerifiedCredential};

enum AdmissionCredential {
    TimeBound(VerifiedCredential),
    LegacyApiKey(UserJwtClaims),
}

impl AdmissionCredential {
    fn into_parts(self) -> (UserJwtClaims, AdmissionValidity) {
        match self {
            Self::TimeBound(verified) => {
                let (claims, expiry) = verified.into_parts();
                (claims, AdmissionValidity::Source(expiry))
            }
            Self::LegacyApiKey(claims) => (claims, AdmissionValidity::LegacyApiKey),
        }
    }
}

enum AdmissionValidity {
    Source(CredentialExpiry),
    LegacyApiKey,
}

/// Trait that the subscriber loop delegates auth decisions to.
#[async_trait::async_trait]
pub trait AuthDispatcher: Send + Sync + 'static {
    async fn dispatch(&self, request: ServerAuthRequestClaims) -> Result<MintedUserJwt, AuthCalloutError>;
}

#[async_trait::async_trait]
impl<D: AuthDispatcher + ?Sized> AuthDispatcher for Arc<D> {
    async fn dispatch(&self, request: ServerAuthRequestClaims) -> Result<MintedUserJwt, AuthCalloutError> {
        self.as_ref().dispatch(request).await
    }
}

pub struct CalloutDispatcherConfig {
    pub tenant_signing_keys: TenantSigningKeys,
    pub user_jwt_ttl: Duration,
    pub a2a_prefix: A2aPrefix,
    pub oidc: Option<Arc<dyn OidcVerifier>>,
    pub mtls: Option<Arc<dyn MTlsVerifier>>,
    #[allow(deprecated)]
    pub api_key: Option<Arc<dyn ApiKeyVerifier>>,
}

pub struct CalloutDispatcher<C = SystemClock> {
    config: CalloutDispatcherConfig,
    clock: C,
}

impl CalloutDispatcher<SystemClock> {
    pub fn new(config: CalloutDispatcherConfig) -> Self {
        Self {
            config,
            clock: SystemClock,
        }
    }
}

impl<C: EpochClock> CalloutDispatcher<C> {
    pub fn with_clock<D: EpochClock>(self, clock: D) -> CalloutDispatcher<D> {
        CalloutDispatcher {
            config: self.config,
            clock,
        }
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
impl<C: EpochClock> AuthDispatcher for CalloutDispatcher<C> {
    async fn dispatch(&self, request: ServerAuthRequestClaims) -> Result<MintedUserJwt, AuthCalloutError> {
        let requested = request.requested_account()?;
        let (tenant, signing_key) = self.tenant(&requested)?;
        let account = AccountName::new(tenant.as_str());

        let verified = match request.presented_credential()? {
            PresentedCredential::BearerToken(token) => {
                let verifier = self
                    .config
                    .oidc
                    .as_ref()
                    .ok_or(CredentialError::VerifierUnavailable { scheme: "OIDC" })?;
                AdmissionCredential::TimeBound(verifier.verify(&token, &account).await?)
            }
            PresentedCredential::ClientCertificate(pem) => {
                let verifier = self
                    .config
                    .mtls
                    .as_ref()
                    .ok_or(CredentialError::VerifierUnavailable { scheme: "mTLS" })?;
                AdmissionCredential::TimeBound(verifier.verify(&pem, &account).await?)
            }
            PresentedCredential::ApiKey(key) => {
                #[allow(deprecated)]
                let verifier = self
                    .config
                    .api_key
                    .as_ref()
                    .ok_or(CredentialError::VerifierUnavailable { scheme: "API-key" })?;
                #[allow(deprecated)]
                AdmissionCredential::LegacyApiKey(verifier.verify(key.as_str(), &account).await?)
            }
        };
        let (claims, validity) = verified.into_parts();
        let principal = claims
            .subject_principal()
            .map_err(CredentialError::UnplaceablePrincipal)?;
        let claims = UserJwtClaims {
            nats_permissions: CallerGrant::new(&self.config.a2a_prefix, &principal)
                .permissions(request.admission_route()),
            ..claims
        };

        let user_nkey = request.user_nkey()?;
        let user_subject = UserJwtSubject::from_user_nkey(user_nkey);
        let material = MintingMaterial::on_behalf_of(signing_key, tenant);
        let issued_at = self.clock.system_time();
        let ttl = match validity {
            AdmissionValidity::Source(expiry) => expiry
                .bounded_ttl(issued_at, self.config.user_jwt_ttl)
                .map_err(CredentialError::Validity)?,
            AdmissionValidity::LegacyApiKey => self.config.user_jwt_ttl,
        };
        claims
            .mint(&material, &user_subject, issued_at, ttl)
            .map_err(AuthCalloutError::from)
    }
}

#[cfg(test)]
#[allow(deprecated)]
pub(crate) mod tests;
