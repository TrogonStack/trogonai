use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use tracing::warn;

use crate::cache::TrustBundleCache;
use crate::error::StsError;
use crate::spiffe_id::SpiffeId;
use crate::svid_verify::workload_svid_from_pem;
use crate::trust::verify_actor_token;
use crate::workload_svid::WorkloadSvid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedCreds {
    pub actor_token: String,
    pub peer_cert_pem: Option<String>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AttestError {
    #[error("attestation denied: {0}")]
    Denied(String),
    #[error("attestation unavailable: {0}")]
    Unavailable(String),
}

impl From<StsError> for AttestError {
    fn from(value: StsError) -> Self {
        match &value {
            StsError::ServerError(_) | StsError::DependencyUnavailable(_) => {
                Self::Unavailable(value.to_string())
            }
            _ => Self::Denied(value.to_string()),
        }
    }
}

impl From<AttestError> for StsError {
    fn from(value: AttestError) -> Self {
        match value {
            AttestError::Denied(msg) => Self::InvalidGrant(msg),
            AttestError::Unavailable(msg) => Self::ServerError(msg),
        }
    }
}

#[async_trait]
pub trait WorkloadAttestor: Send + Sync {
    async fn attest(&self, presented: &PresentedCreds) -> Result<WorkloadSvid, AttestError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttestationPolicy {
    Require,
    Shadow,
}

impl AttestationPolicy {
    pub fn from_env() -> Self {
        match std::env::var("MCP_STS_REQUIRE_ATTESTATION") {
            Ok(v) if v == "1" || v.eq_ignore_ascii_case("true") => Self::Require,
            _ => Self::Shadow,
        }
    }

    #[must_use]
    pub fn is_shadow(&self) -> bool {
        matches!(self, Self::Shadow)
    }
}

pub struct X509SvidAttestor {
    trust_bundle: TrustBundleCache,
}

/// NATS STS ingress is request/reply without mTLS; peer certs arrive via `actor_token` PEM until HTTP ingress lands.
pub type MtlsSvidAttestor = X509SvidAttestor;

impl X509SvidAttestor {
    pub fn new(trust_bundle: TrustBundleCache) -> Self {
        Self { trust_bundle }
    }
}

#[async_trait]
impl WorkloadAttestor for X509SvidAttestor {
    async fn attest(&self, presented: &PresentedCreds) -> Result<WorkloadSvid, AttestError> {
        let material = presented
            .peer_cert_pem
            .as_deref()
            .unwrap_or(presented.actor_token.as_str());
        let trust_domain = infer_trust_domain(material)?;
        let bundle_pem = self
            .trust_bundle
            .pem_for_domain(&trust_domain)
            .await
            .map_err(|e| AttestError::Unavailable(e.to_string()))?;
        workload_svid_from_pem(material, &bundle_pem).map_err(Into::into)
    }
}

pub struct NoOpAttestor {
    trust_bundle: TrustBundleCache,
    policy: AttestationPolicy,
    warned: std::sync::atomic::AtomicBool,
}

impl NoOpAttestor {
    pub fn new(trust_bundle: TrustBundleCache, policy: AttestationPolicy) -> Self {
        if policy.is_shadow() {
            warn!(
                "MCP_STS_REQUIRE_ATTESTATION is unset: actor_token wkl is claim-based (shadow); \
                 deny audits will emit reason wkl_unattested"
            );
        }
        Self {
            trust_bundle,
            policy,
            warned: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn log_startup_warning(&self) {
        if self
            .warned
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
        {
            warn!(
                "NoOp workload attestor active: X.509 SVID chain verification is bypassed for \
                 spiffe:// and sha256: actor tokens"
            );
        }
    }
}

#[async_trait]
impl WorkloadAttestor for NoOpAttestor {
    async fn attest(&self, presented: &PresentedCreds) -> Result<WorkloadSvid, AttestError> {
        if self.policy == AttestationPolicy::Require {
            return Err(AttestError::Denied(
                "MCP_STS_REQUIRE_ATTESTATION=1 but no X.509 SVID attestor is configured".into(),
            ));
        }
        self.log_startup_warning();
        let bundle_pem = self
            .trust_bundle
            .pem()
            .await
            .map_err(|e| AttestError::Unavailable(e.to_string()))?;
        let wkl = verify_actor_token(&presented.actor_token, &bundle_pem)
            .map_err(|e| AttestError::Denied(e.to_string()))?;
        let spiffe_id = SpiffeId::parse(&wkl).map_err(|e| AttestError::Denied(e.to_string()))?;
        Ok(WorkloadSvid::new(
            spiffe_id,
            Vec::new(),
            presented.actor_token.clone(),
        ))
    }
}

pub type DynAttestor = Arc<dyn WorkloadAttestor>;

pub fn build_attestor(trust_bundle: TrustBundleCache, policy: AttestationPolicy) -> DynAttestor {
    match policy {
        AttestationPolicy::Require => Arc::new(X509SvidAttestor::new(trust_bundle)),
        AttestationPolicy::Shadow => Arc::new(NoOpAttestor::new(trust_bundle, policy)),
    }
}

fn infer_trust_domain(material: &str) -> Result<String, AttestError> {
    let trimmed = material.trim();
    if trimmed.starts_with("spiffe://") {
        return SpiffeId::parse(trimmed)
            .map(|id| id.trust_domain().to_string())
            .map_err(|e| AttestError::Denied(e.to_string()));
    }
    let leaf_der = crate::svid_verify::leaf_der_from_actor_token(trimmed)?;
    let spiffe_id = crate::svid_verify::spiffe_id_from_leaf_der(&leaf_der)?;
    Ok(spiffe_id.trust_domain().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::TrustBundleCache;

    #[tokio::test]
    async fn noop_rejects_when_require_attestation() {
        let trust = TrustBundleCache::from_pem("-----BEGIN TRUST BUNDLE-----".into());
        let attestor = NoOpAttestor::new(trust, AttestationPolicy::Require);
        let err = attestor
            .attest(&PresentedCreds {
                actor_token: "spiffe://acme.local/ns/prod/sa/x".into(),
                peer_cert_pem: None,
            })
            .await
            .expect_err("require");
        assert!(matches!(err, AttestError::Denied(_)));
    }

    #[tokio::test]
    async fn noop_shadow_accepts_spiffe_uri() {
        let trust = TrustBundleCache::from_pem("-----BEGIN TRUST BUNDLE-----".into());
        let attestor = NoOpAttestor::new(trust, AttestationPolicy::Shadow);
        let svid = attestor
            .attest(&PresentedCreds {
                actor_token: "spiffe://acme.local/ns/prod/sa/oncall-agent".into(),
                peer_cert_pem: None,
            })
            .await
            .expect("shadow");
        assert_eq!(svid.wkl(), "spiffe://acme.local/ns/prod/sa/oncall-agent");
    }
}
