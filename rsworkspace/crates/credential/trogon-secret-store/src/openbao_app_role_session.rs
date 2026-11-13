use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use tokio::sync::Mutex;
use tracing::warn;
use trogon_std::{GetNow, SecretString};
use url::Url;

use super::constants::{APPROLE_LOGIN_PATH, TOKEN_HEADER, TOKEN_RENEW_SELF_PATH};
use super::openbao_http::send_token_json;
use super::{OpenBaoAppRole, OpenBaoAuthResponse, OpenBaoCallError, OpenBaoLease, SecretStoreError};

/// The clock a session measures its lease against.
pub(crate) type OpenBaoClock = Arc<dyn GetNow<Instant = Instant> + Send + Sync>;

/// An AppRole login held for as long as the process runs.
///
/// The token is renewed ahead of its expiry and replaced by a fresh login when
/// renewal is refused or the role's max TTL caps it. A token past its usable
/// life is never sent: when no fresh one can be had, the caller gets
/// [`SecretStoreError::BackendUnavailable`] instead.
pub(crate) struct OpenBaoAppRoleSession {
    approle: OpenBaoAppRole,
    clock: OpenBaoClock,
    lease: Mutex<Option<OpenBaoLease>>,
}

impl OpenBaoAppRoleSession {
    /// Logs in once, so a service that cannot authenticate refuses to start.
    pub(crate) async fn start(
        approle: OpenBaoAppRole,
        clock: OpenBaoClock,
        http: &reqwest::Client,
        address: &Url,
    ) -> Result<Self, OpenBaoCallError> {
        let session = Self {
            approle,
            clock,
            lease: Mutex::new(None),
        };
        let lease = session.login(http, address).await?;
        *session.lease.lock().await = Some(lease);
        Ok(session)
    }

    pub(crate) async fn token(&self, http: &reqwest::Client, address: &Url) -> Result<SecretString, SecretStoreError> {
        let mut held = self.lease.lock().await;
        if let Some(lease) = held.as_ref()
            && !lease.is_renewal_due(self.clock.now())
        {
            return Ok(lease.token().clone());
        }
        let lease = self
            .refresh(held.take(), http, address)
            .await
            .map_err(OpenBaoCallError::into_store_error)?;
        let token = lease.token().clone();
        *held = Some(lease);
        Ok(token)
    }

    async fn refresh(
        &self,
        previous: Option<OpenBaoLease>,
        http: &reqwest::Client,
        address: &Url,
    ) -> Result<OpenBaoLease, OpenBaoCallError> {
        if let Some(previous) = previous
            && previous.can_renew(self.clock.now())
        {
            match self.renew(&previous, http, address).await {
                Ok(renewed) if renewed.ttl() >= previous.ttl() => return Ok(renewed),
                Ok(renewed) => warn!(
                    granted_secs = renewed.ttl().as_secs(),
                    requested_secs = previous.ttl().as_secs(),
                    "OpenBao capped the login token renewal at its max TTL; logging in again"
                ),
                Err(error) => warn!(error = %error, "OpenBao refused to renew the login token; logging in again"),
            }
        }
        self.login(http, address).await
    }

    async fn login(&self, http: &reqwest::Client, address: &Url) -> Result<OpenBaoLease, OpenBaoCallError> {
        let url = address
            .join(APPROLE_LOGIN_PATH)
            .map_err(OpenBaoCallError::InvalidEndpoint)?;
        let obtained_at = self.clock.now();
        let response: OpenBaoAuthResponse = send_token_json(http.post(url).json(&OpenBaoLoginRequest {
            role_id: self.approle.role_id(),
            secret_id: self.approle.secret_id().as_str(),
        }))
        .await?;
        self.admit(response.into_lease(obtained_at))
    }

    async fn renew(
        &self,
        lease: &OpenBaoLease,
        http: &reqwest::Client,
        address: &Url,
    ) -> Result<OpenBaoLease, OpenBaoCallError> {
        let url = address
            .join(TOKEN_RENEW_SELF_PATH)
            .map_err(OpenBaoCallError::InvalidEndpoint)?;
        let obtained_at = self.clock.now();
        let response: OpenBaoAuthResponse = send_token_json(
            http.post(url)
                .header(TOKEN_HEADER, lease.token().as_str())
                .json(&OpenBaoRenewRequest {
                    increment: format!("{}s", lease.ttl().as_secs()),
                }),
        )
        .await?;
        self.admit(response.into_lease(obtained_at))
    }

    fn admit(&self, lease: OpenBaoLease) -> Result<OpenBaoLease, OpenBaoCallError> {
        if !lease.is_usable(self.clock.now()) {
            return Err(OpenBaoCallError::UnusableLease { ttl: lease.ttl() });
        }
        Ok(lease)
    }
}

impl fmt::Debug for OpenBaoAppRoleSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenBaoAppRoleSession")
            .field("approle", &self.approle)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize)]
struct OpenBaoLoginRequest<'a> {
    role_id: &'a str,
    secret_id: &'a str,
}

#[derive(Serialize)]
struct OpenBaoRenewRequest {
    increment: String,
}

#[cfg(test)]
mod tests;
