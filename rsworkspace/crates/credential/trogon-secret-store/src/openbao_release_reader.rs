use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde::Serialize;
use trogon_credential_domain::{AuditCorrelationId, CredentialRef};
use trogon_std::{SecretString, SystemClock};

use super::constants::{
    CHILD_TOKEN_DISPLAY_NAME, CHILD_TOKEN_TTL, CHILD_TOKEN_USES, CORRELATION_ID_METADATA_KEY, TOKEN_CREATE_PATH,
};
use super::openbao_app_role_session::OpenBaoClock;
use super::openbao_http::send_token_json;
use super::{
    OpenBaoAppRole, OpenBaoAuthResponse, OpenBaoCallError, OpenBaoCaller, OpenBaoChildTokens, OpenBaoLoginError,
    OpenBaoMount, OpenBaoSecretStore, OpenBaoTransport, SecretMaterial, SecretStoreCorrelatedGet, SecretStoreError,
};

/// Reads credential material for the secrets service's releases.
///
/// It logs in with an AppRole only, so the service never runs on a static
/// token. The login token can do nothing but mint children through one token
/// role and keep itself alive; each read mints its own child, good for that
/// read alone and carrying the release's correlation id in its metadata, so
/// OpenBao's audit log ties the read to the release, per ADR#0023 Decisions 3
/// and 6.
#[derive(Clone)]
pub struct OpenBaoReleaseReader {
    store: OpenBaoSecretStore,
    child_tokens: OpenBaoChildTokens,
}

impl OpenBaoReleaseReader {
    pub async fn login(
        address: impl AsRef<str>,
        transport: OpenBaoTransport,
        mount: OpenBaoMount,
        approle: OpenBaoAppRole,
        child_tokens: OpenBaoChildTokens,
    ) -> Result<Self, OpenBaoLoginError> {
        Self::login_with_clock(address, transport, mount, approle, child_tokens, Arc::new(SystemClock)).await
    }

    pub(crate) async fn login_with_clock(
        address: impl AsRef<str>,
        transport: OpenBaoTransport,
        mount: OpenBaoMount,
        approle: OpenBaoAppRole,
        child_tokens: OpenBaoChildTokens,
        clock: OpenBaoClock,
    ) -> Result<Self, OpenBaoLoginError> {
        let store = OpenBaoSecretStore::login(address, transport, mount, approle, clock).await?;
        Ok(Self { store, child_tokens })
    }

    async fn mint_child(
        &self,
        login: &OpenBaoCaller,
        correlation_id: &AuditCorrelationId,
    ) -> Result<SecretString, OpenBaoCallError> {
        let url = self
            .store
            .address()
            .join(&format!("{TOKEN_CREATE_PATH}/{}", self.child_tokens.role()))
            .map_err(OpenBaoCallError::InvalidEndpoint)?;
        let response: OpenBaoAuthResponse = send_token_json(
            login
                .authorize(self.store.http().post(url))
                .json(&OpenBaoChildTokenRequest::new(&self.child_tokens, correlation_id)),
        )
        .await?;
        Ok(response.into_token())
    }
}

impl SecretStoreCorrelatedGet for OpenBaoReleaseReader {
    type Error = SecretStoreError;

    async fn get_correlated(
        &self,
        credential: &CredentialRef,
        correlation_id: &AuditCorrelationId,
    ) -> Result<SecretMaterial, Self::Error> {
        let login = OpenBaoCaller::correlated(self.store.token().await?, correlation_id.clone());
        let child = self
            .mint_child(&login, correlation_id)
            .await
            .map_err(OpenBaoCallError::into_store_error)?;
        self.store
            .read_material(credential, &OpenBaoCaller::correlated(child, correlation_id.clone()))
            .await
    }
}

impl fmt::Debug for OpenBaoReleaseReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenBaoReleaseReader")
            .field("store", &self.store)
            .field("child_tokens", &self.child_tokens)
            .finish()
    }
}

#[derive(Serialize)]
struct OpenBaoChildTokenRequest<'a> {
    policies: [&'a str; 1],
    no_default_policy: bool,
    ttl: String,
    num_uses: u32,
    renewable: bool,
    display_name: &'static str,
    meta: BTreeMap<&'static str, &'a str>,
}

impl<'a> OpenBaoChildTokenRequest<'a> {
    fn new(child_tokens: &'a OpenBaoChildTokens, correlation_id: &'a AuditCorrelationId) -> Self {
        Self {
            policies: [child_tokens.policy().as_str()],
            no_default_policy: true,
            ttl: format!("{}s", CHILD_TOKEN_TTL.as_secs()),
            num_uses: CHILD_TOKEN_USES,
            renewable: false,
            display_name: CHILD_TOKEN_DISPLAY_NAME,
            meta: BTreeMap::from([(CORRELATION_ID_METADATA_KEY, correlation_id.as_str())]),
        }
    }
}

#[cfg(test)]
mod tests;
