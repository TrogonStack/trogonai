use std::num::NonZeroU64;

use trogon_audit::AuditName;
use trogon_nats::{NatsAuth, NatsConfig, NatsTls, NatsToken, TlsMisconfiguration};

use crate::scoped_stream::is_exclusive_snapshot;
use crate::{ScopedSnapshotKey, SnapshotBucket, SnapshotBucketError};

#[derive(Debug, Clone)]
pub struct KvPolicyConfig {
    pub(crate) bucket: SnapshotBucket,
    pub(crate) tenant: AuditName,
    pub(crate) project: NatsToken,
    pub(crate) incarnation: NonZeroU64,
    key: ScopedSnapshotKey,
    isolation: ScopeIsolation,
}

#[derive(Debug, Clone, Copy)]
enum ScopeIsolation {
    ConfiguredKey,
    ExclusiveStream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PolicyConfigError {
    #[error("policy bucket must be a bounded NATS bucket name")]
    Bucket(#[source] SnapshotBucketError),
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
        let bucket = SnapshotBucket::new(bucket).map_err(PolicyConfigError::Bucket)?;
        let key = ScopedSnapshotKey::new(&tenant, &project);
        Ok(Self {
            bucket,
            tenant,
            project,
            incarnation,
            key,
            isolation: ScopeIsolation::ConfiguredKey,
        })
    }

    pub fn bucket(&self) -> &str {
        self.bucket.as_str()
    }

    pub(crate) fn matches_scope(&self, tenant: &AuditName, project: &NatsToken) -> bool {
        &self.tenant == tenant && &self.project == project
    }
    pub fn key(&self) -> &str {
        self.key.as_str()
    }

    pub(crate) fn for_workload_identity(&self) -> Self {
        let mut config = self.clone();
        config.isolation = ScopeIsolation::ExclusiveStream;
        config
    }

    pub(crate) fn stream_matches_scope(&self, info: &async_nats::jetstream::stream::Info) -> bool {
        match self.isolation {
            ScopeIsolation::ConfiguredKey => true,
            ScopeIsolation::ExclusiveStream => {
                is_exclusive_snapshot(info, &format!("$KV.{}.{}", self.bucket, self.key))
            }
        }
    }
}
