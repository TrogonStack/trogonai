use std::fmt;

use nats_jwt_rs::Claims;
use nats_jwt_rs::authorization::AuthRequest;
use serde::Deserialize;

use super::NkeyPublic;
use crate::account_resolver::RequestedAccount;
use crate::credentials::PresentedCredential;
use crate::credentials::mtls::ClientCertPem;
use crate::error::{AuthCalloutError, CredentialError};

/// Decoded inner authorization-request JWT (`nats` claims + standard JWT fields).
#[derive(Clone)]
pub struct ServerAuthRequestClaims {
    inner: Claims<AuthRequest>,
    nats_json: serde_json::Value,
    declared_credential: Option<PresentedCredential>,
}

impl ServerAuthRequestClaims {
    pub(crate) fn from_decoded(inner: Claims<AuthRequest>, nats_json: serde_json::Value) -> Self {
        Self {
            inner,
            nats_json,
            declared_credential: None,
        }
    }

    pub(super) fn declaring(
        inner: Claims<AuthRequest>,
        nats_json: serde_json::Value,
        declared_credential: Option<PresentedCredential>,
    ) -> Self {
        Self {
            inner,
            nats_json,
            declared_credential,
        }
    }

    pub fn issuer_nkey(&self) -> Result<NkeyPublic, AuthCalloutError> {
        NkeyPublic::parse(&self.inner.iss)
    }

    pub fn user_nkey(&self) -> Result<NkeyPublic, AuthCalloutError> {
        NkeyPublic::parse(&self.inner.nats.user_nkey)
    }

    pub fn server_id(&self) -> &str {
        &self.inner.nats.server.id
    }

    pub fn server_one_time_xkey(&self) -> Option<&str> {
        self.inner.nats.server.xkey.as_deref().filter(|xkey| !xkey.is_empty())
    }

    /// A tenant caller connects as the `AUTH` sentinel, so `client_info`
    /// describes the sentinel and only `connect_opts.user` names the tenant.
    pub fn requested_account(&self) -> Result<RequestedAccount, AuthCalloutError> {
        let user = self
            .inner
            .nats
            .connect_opts
            .user
            .as_deref()
            .filter(|user| !user.trim().is_empty())
            .ok_or_else(|| {
                CredentialError::InvalidCredentials(
                    "authorization request does not name its tenant account in connect_opts.user".into(),
                )
            })?;
        RequestedAccount::new(user.to_owned()).map_err(AuthCalloutError::from)
    }

    /// `connect_opts.jwt` always holds the sentinel, so the caller's own
    /// credential is the secret in `auth_token`, then `pass`, and the client
    /// certificate only when no secret is presented.
    pub fn presented_credential(&self) -> Result<PresentedCredential, AuthCalloutError> {
        if let Some(credential) = &self.declared_credential {
            return Ok(credential.clone());
        }
        let connect_opts = &self.inner.nats.connect_opts;
        let secret = [&connect_opts.auth_token, &connect_opts.pass]
            .into_iter()
            .filter_map(Option::as_deref)
            .find(|secret| !secret.trim().is_empty());
        if let Some(secret) = secret {
            return PresentedCredential::from_secret(secret).map_err(AuthCalloutError::from);
        }
        self.primary_client_cert()
            .map(PresentedCredential::ClientCertificate)
            .ok_or_else(|| {
                CredentialError::InvalidRequest("authorization request presents no credential".into()).into()
            })
    }

    pub fn client_tls_pem_certs(&self) -> Vec<ClientCertPem> {
        #[derive(Deserialize)]
        struct Tls {
            certs: Option<Vec<String>>,
            verified_chains: Option<Vec<Vec<String>>>,
        }
        let tls: Option<Tls> = match self.nats_json.get("client_tls") {
            Some(v) => serde_json::from_value(v.clone()).ok(),
            None => None,
        };
        let tls = tls.filter(|t| {
            t.certs.as_ref().is_some_and(|c| !c.is_empty()) || t.verified_chains.as_ref().is_some_and(|c| !c.is_empty())
        });
        let Some(tls) = tls else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if let Some(certs) = tls.certs {
            for pem in certs {
                out.push(ClientCertPem::new(pem));
            }
        }
        if let Some(chains) = tls.verified_chains {
            for chain in chains {
                for pem in chain {
                    out.push(ClientCertPem::new(pem));
                }
            }
        }
        out
    }

    pub fn primary_client_cert(&self) -> Option<ClientCertPem> {
        self.client_tls_pem_certs().into_iter().next()
    }
}

impl fmt::Debug for ServerAuthRequestClaims {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerAuthRequestClaims")
            .field("user_nkey", &self.inner.nats.user_nkey)
            .field("server_id", &self.inner.nats.server.id)
            .finish()
    }
}

#[cfg(test)]
mod tests;
