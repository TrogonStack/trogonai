use std::fmt;

use nats_jwt_rs::Claims;
use nats_jwt_rs::authorization::AuthRequest;
use serde::Deserialize;
use x509_parser::pem::Pem;

use super::NkeyPublic;
use crate::account_resolver::RequestedAccount;
use crate::credentials::PresentedCredential;
use crate::credentials::mtls::ClientCertPem;
use crate::error::{AuthCalloutError, CredentialError};
use crate::permissions::AdmissionRoute;

/// Decoded inner authorization-request JWT (`nats` claims + standard JWT fields).
#[derive(Clone)]
pub struct ServerAuthRequestClaims {
    inner: Claims<AuthRequest>,
    nats_json: serde_json::Value,
    declared_credential: Option<PresentedCredential>,
    route: AdmissionRoute,
}

impl ServerAuthRequestClaims {
    pub(crate) fn from_decoded(inner: Claims<AuthRequest>, nats_json: serde_json::Value) -> Self {
        Self {
            inner,
            nats_json,
            declared_credential: None,
            route: AdmissionRoute::Direct,
        }
    }

    pub(super) fn bridge_routed(
        inner: Claims<AuthRequest>,
        nats_json: serde_json::Value,
        declared_credential: Option<PresentedCredential>,
    ) -> Self {
        Self {
            inner,
            nats_json,
            declared_credential,
            route: AdmissionRoute::BridgeRouted,
        }
    }

    pub fn admission_route(&self) -> AdmissionRoute {
        self.route
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
        struct ClientTlsWire {
            certs: Option<Vec<String>>,
            verified_chains: Option<Vec<Vec<String>>>,
        }
        let tls: Option<ClientTlsWire> = match self.nats_json.get("client_tls") {
            Some(v) => serde_json::from_value(v.clone()).ok(),
            None => None,
        };
        let Some(tls) = tls else {
            return Vec::new();
        };
        let verified = tls.verified_chains.unwrap_or_default();
        let selected = tls
            .certs
            .filter(|certs| !certs.is_empty())
            .or_else(|| verified.first().cloned());
        let Some(selected) = selected else {
            return Vec::new();
        };
        let Some(leaf) = selected.first() else {
            return Vec::new();
        };
        if selected.iter().any(|pem| pem.trim().is_empty())
            || verified
                .iter()
                .any(|chain| chain.first().is_none_or(|other| !same_leaf_certificate(leaf, other)))
        {
            return Vec::new();
        }
        selected.into_iter().map(ClientCertPem::new).collect()
    }

    pub fn primary_client_cert(&self) -> Option<ClientCertPem> {
        let chain = self.client_tls_pem_certs();
        let mut pem = chain.first()?.as_str().to_owned();
        for certificate in &chain[1..] {
            pem.push('\n');
            pem.push_str(certificate.as_str());
        }
        Some(ClientCertPem::new(pem))
    }
}

fn same_leaf_certificate(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let leaf_der = |pem: &str| {
        Pem::iter_from_buffer(pem.as_bytes())
            .next()?
            .ok()
            .filter(|certificate| certificate.label == "CERTIFICATE")
            .map(|certificate| certificate.contents)
    };
    matches!((leaf_der(left), leaf_der(right)), (Some(left), Some(right)) if left == right)
}

impl fmt::Debug for ServerAuthRequestClaims {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerAuthRequestClaims")
            .field("user_nkey", &self.inner.nats.user_nkey)
            .field("server_id", &self.inner.nats.server.id)
            .field("route", &self.route)
            .finish()
    }
}

#[cfg(test)]
mod tests;
