use std::fmt;

use nats_jwt_rs::authorization::AuthRequest;
use nats_jwt_rs::Claims;
use serde::Deserialize;

use super::NkeyPublic;
use crate::account_resolver::RequestedAccount;
use crate::credentials::mtls::ClientCertPem;
use crate::error::AuthCalloutError;

/// Decoded inner authorization-request JWT (`nats` claims + standard JWT fields).
#[derive(Clone)]
pub struct ServerAuthRequestClaims {
    inner: Claims<AuthRequest>,
    nats_json: serde_json::Value,
}

impl ServerAuthRequestClaims {
    pub(crate) fn from_decoded(inner: Claims<AuthRequest>, nats_json: serde_json::Value) -> Self {
        Self { inner, nats_json }
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
        self.inner.nats.server.xkey.as_deref()
    }

    pub fn requested_account(&self) -> Result<RequestedAccount, AuthCalloutError> {
        let hint = self
            .inner
            .nats
            .connect_opts
            .user
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                let u = self.inner.nats.client_info.user.as_str();
                if u.is_empty() { None } else { Some(u) }
            })
            .or_else(|| {
                let tag = self.inner.nats.client_info.name_tag.as_str();
                if tag.is_empty() { None } else { Some(tag) }
            });
        let hint = hint.ok_or_else(|| {
            AuthCalloutError::CredentialVerification(
                "authorization request missing tenant account hint (connect_opts.user, client_info.user, or name_tag)".into(),
            )
        })?;
        RequestedAccount::new(hint.to_owned()).map_err(AuthCalloutError::from)
    }

    pub fn connect_opts_jwt(&self) -> Option<&str> {
        non_empty_opt(self.inner.nats.connect_opts.jwt.as_deref()).or_else(|| {
            self.nats_json
                .get("connect_opts")?
                .get("jwt")?
                .as_str()
                .filter(|s| !s.is_empty())
        })
    }

    pub fn connect_opts_opaque_pass(&self) -> Option<&str> {
        non_empty_opt(self.inner.nats.connect_opts.pass.as_deref()).or_else(|| {
            self.nats_json
                .get("connect_opts")?
                .get("pass")?
                .as_str()
                .filter(|s| !s.is_empty())
        })
    }

    pub fn connect_opts_auth_token(&self) -> Option<&str> {
        self.inner.nats.connect_opts.auth_token.as_deref()
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
            t.certs.as_ref().is_some_and(|c| !c.is_empty())
                || t.verified_chains.as_ref().is_some_and(|c| !c.is_empty())
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

fn non_empty_opt(value: Option<&str>) -> Option<&str> {
    value.filter(|s| !s.is_empty())
}

impl fmt::Debug for ServerAuthRequestClaims {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerAuthRequestClaims")
            .field("user_nkey", &self.inner.nats.user_nkey)
            .field("server_id", &self.inner.nats.server.id)
            .finish()
    }
}
