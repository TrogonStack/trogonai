use std::num::NonZeroU64;

use trogon_workload_identity::{
    IdentityConfig, RegistrationBinding, RegistrationError, UnixEndpoint, UnixEndpointError, WorkloadRole,
};

use crate::audit_config::AuditConfig;
use crate::trust_domain::TrustDomain;
use crate::{ScopedSnapshotKey, SnapshotBucket, SnapshotBucketError};

#[derive(Debug, Clone)]
pub struct WorkloadIdentityConfig {
    pub(crate) identity: IdentityConfig,
    registration_bucket: SnapshotBucket,
    registration_key: ScopedSnapshotKey,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkloadIdentityConfigError {
    #[error("workload identity endpoint is not an absolute Unix socket")]
    Endpoint(#[source] UnixEndpointError),
    #[error("workload registration is invalid")]
    Registration(#[source] RegistrationError),
    #[error("workload identity is outside the configured trust domain")]
    TrustDomain,
    #[error("registration bucket is not a bounded NATS bucket name")]
    Bucket(#[source] SnapshotBucketError),
}

impl WorkloadIdentityConfig {
    pub fn new(
        endpoint: &str,
        identity: &str,
        registration_bucket: &str,
        recovery_fence: NonZeroU64,
        trust_domain: &TrustDomain,
        audit: &AuditConfig,
    ) -> Result<Self, WorkloadIdentityConfigError> {
        let endpoint = UnixEndpoint::new(endpoint).map_err(WorkloadIdentityConfigError::Endpoint)?;
        let local = RegistrationBinding::new(
            identity,
            audit.identity.tenant().as_str(),
            audit.identity.project().as_str(),
            WorkloadRole::EgressProxy,
            None,
        )
        .map_err(WorkloadIdentityConfigError::Registration)?;
        if local.spiffe_id().trust_domain().to_string() != trust_domain.as_str() {
            return Err(WorkloadIdentityConfigError::TrustDomain);
        }
        let registration_bucket =
            SnapshotBucket::new(registration_bucket).map_err(WorkloadIdentityConfigError::Bucket)?;
        Ok(Self {
            identity: IdentityConfig::new(endpoint, local, recovery_fence),
            registration_bucket,
            registration_key: ScopedSnapshotKey::new(audit.identity.tenant(), audit.identity.project()),
        })
    }

    pub fn identity(&self) -> &IdentityConfig {
        &self.identity
    }

    pub fn registration_bucket(&self) -> &str {
        self.registration_bucket.as_str()
    }

    pub fn registration_key(&self) -> &str {
        self.registration_key.as_str()
    }
}
