use std::num::NonZeroU64;

use trogon_nats::{DottedNatsToken, SubjectTokenViolationError};

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
    reply_inbox_prefix: DottedNatsToken,
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
    #[error("workload reply inbox prefix is not a bounded NATS namespace")]
    ReplyPrefix(#[source] SubjectTokenViolationError),
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
        let reply_inbox_prefix = DottedNatsToken::new("_INBOX").map_err(WorkloadIdentityConfigError::ReplyPrefix)?;
        Ok(Self {
            identity: IdentityConfig::new(endpoint, local, recovery_fence),
            registration_bucket,
            registration_key: ScopedSnapshotKey::new(audit.identity.tenant(), audit.identity.project()),
            reply_inbox_prefix,
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

    pub fn reply_inbox_prefix(&self) -> &DottedNatsToken {
        &self.reply_inbox_prefix
    }
}
