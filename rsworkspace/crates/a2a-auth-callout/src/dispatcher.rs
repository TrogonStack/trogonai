use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::account_resolver::AccountResolver;
#[allow(deprecated)]
use crate::credentials::api_key::ApiKeyVerifier;
use crate::credentials::mtls::MTlsVerifier;
use crate::credentials::oidc::{BearerToken, OidcVerifier};
use crate::error::AuthCalloutError;
use crate::jwt::{MintedUserJwt, UserJwtSubject};
use crate::signing_key_source::SigningKeySource;
use crate::wire::ServerAuthRequestClaims;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthScheme {
    Oidc,
    MTls,
    ApiKey,
}

/// Trait that the subscriber loop delegates auth decisions to.
#[async_trait::async_trait]
pub trait AuthDispatcher: Send + Sync + 'static {
    async fn dispatch(&self, request: ServerAuthRequestClaims) -> Result<MintedUserJwt, AuthCalloutError>;
}

pub struct CalloutDispatcherConfig {
    pub signing_key_source: Arc<dyn SigningKeySource>,
    pub user_jwt_ttl: Duration,
    pub account_resolver: Arc<dyn AccountResolver>,
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

    fn select_scheme(&self, request: &ServerAuthRequestClaims) -> Result<AuthScheme, AuthCalloutError> {
        if request.connect_opts_jwt().is_some() || request.connect_opts_opaque_pass().is_some() {
            return Ok(AuthScheme::Oidc);
        }
        if request.primary_client_cert().is_some() {
            return Ok(AuthScheme::MTls);
        }
        if request.connect_opts_auth_token().is_some() {
            return Ok(AuthScheme::ApiKey);
        }
        Err(
            crate::error::CredentialError::InvalidRequest("no credential material in authorization request".into())
                .into(),
        )
    }
}

#[async_trait::async_trait]
impl AuthDispatcher for CalloutDispatcher {
    async fn dispatch(&self, request: ServerAuthRequestClaims) -> Result<MintedUserJwt, AuthCalloutError> {
        let requested = request.requested_account()?;
        let account = self.config.account_resolver.resolve(&requested)?;

        let scheme = self.select_scheme(&request)?;
        let claims = match scheme {
            AuthScheme::Oidc => {
                let verifier = self
                    .config
                    .oidc
                    .as_ref()
                    .ok_or(crate::error::CredentialError::VerifierUnavailable { scheme: "OIDC" })?;
                let token = request
                    .connect_opts_jwt()
                    .or_else(|| request.connect_opts_opaque_pass())
                    .ok_or_else(|| {
                        crate::error::CredentialError::InvalidRequest(
                            "OIDC scheme but connect_opts.jwt and connect_opts.pass missing".into(),
                        )
                    })?;
                verifier.verify(&BearerToken::new(token.to_owned()), &account).await?
            }
            AuthScheme::MTls => {
                let verifier = self
                    .config
                    .mtls
                    .as_ref()
                    .ok_or(crate::error::CredentialError::VerifierUnavailable { scheme: "mTLS" })?;
                let pem = request.primary_client_cert().ok_or_else(|| {
                    crate::error::CredentialError::InvalidRequest("mTLS scheme but client_tls certs missing".into())
                })?;
                verifier.verify(&pem, &account).await?
            }
            AuthScheme::ApiKey => {
                #[allow(deprecated)]
                let verifier = self
                    .config
                    .api_key
                    .as_ref()
                    .ok_or(crate::error::CredentialError::VerifierUnavailable { scheme: "API-key" })?;
                let key = request.connect_opts_auth_token().ok_or_else(|| {
                    crate::error::CredentialError::InvalidRequest(
                        "API-key scheme but connect_opts.auth_token missing".into(),
                    )
                })?;
                // ApiKeyVerifier::verify now takes the resolved account
                // directly and refuses mismatches itself, so the explicit
                // post-check that was here is now redundant.
                #[allow(deprecated)]
                verifier.verify(key, &account).await?
            }
        };

        let user_nkey = request.user_nkey()?;
        let user_subject = UserJwtSubject::from_user_nkey(user_nkey);
        let material = self.config.signing_key_source.current().minting_material();
        claims
            .mint(&material, &user_subject, SystemTime::now(), self.config.user_jwt_ttl)
            .map_err(AuthCalloutError::from)
    }
}

#[cfg(test)]
#[allow(deprecated)]
pub(crate) mod tests;
