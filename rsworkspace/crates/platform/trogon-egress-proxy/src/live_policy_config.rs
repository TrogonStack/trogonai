use std::num::NonZeroU64;

use trogon_audit::AuditName;
use trogon_nats::{NatsAuth, NatsConfig, NatsTls, NatsToken, TlsMisconfiguration};

use crate::constants::LIVE_POLICY_MAX_BUCKET_BYTES;
use crate::scoped_key::scoped_key;

#[derive(Debug, Clone)]
pub struct KvPolicyConfig {
    pub(crate) bucket: String,
    pub(crate) tenant: AuditName,
    pub(crate) project: NatsToken,
    pub(crate) incarnation: NonZeroU64,
    key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PolicyConfigError {
    #[error("policy bucket must be a bounded NATS bucket name")]
    Bucket,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyTransportError {
    #[error("live policy requires authenticated TLS")]
    TlsRequired,
    #[error("live policy requires client authentication")]
    ClientAuthenticationRequired,
    #[error("live policy TLS configuration is invalid")]
    TlsInvalid(TlsMisconfiguration),
}

impl KvPolicyConfig {
    pub fn validate_transport(nats: &NatsConfig) -> Result<(), PolicyTransportError> {
        match &nats.tls {
            Ok(NatsTls::Enabled { client_cert, .. }) => {
                if client_cert.is_none() && matches!(nats.auth, NatsAuth::None | NatsAuth::Jwt { seed: None, .. }) {
                    return Err(PolicyTransportError::ClientAuthenticationRequired);
                }
                Ok(())
            }
            Ok(NatsTls::Disabled) => Err(PolicyTransportError::TlsRequired),
            Err(error) => Err(PolicyTransportError::TlsInvalid(error.clone())),
        }
    }

    pub fn new(
        bucket: &str,
        tenant: AuditName,
        project: NatsToken,
        incarnation: NonZeroU64,
    ) -> Result<Self, PolicyConfigError> {
        if bucket.is_empty()
            || bucket.len() > LIVE_POLICY_MAX_BUCKET_BYTES
            || !bucket
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(PolicyConfigError::Bucket);
        }
        let key = scoped_key(&tenant, &project);
        Ok(Self {
            bucket: bucket.to_owned(),
            tenant,
            project,
            incarnation,
            key,
        })
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    pub(crate) fn matches_scope(&self, tenant: &AuditName, project: &NatsToken) -> bool {
        &self.tenant == tenant && &self.project == project
    }
    pub fn key(&self) -> &str {
        &self.key
    }
}
