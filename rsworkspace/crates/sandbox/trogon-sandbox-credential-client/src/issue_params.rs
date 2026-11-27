use std::num::NonZeroU32;

use trogon_sandbox_contract::{CredentialScope, PullSecretId, ServiceAccountId};

/// What to ask the broker to issue.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct IssueParams {
    pub pull_secret_id: PullSecretId,
    pub scope: CredentialScope,
    pub ttl_seconds: NonZeroU32,
    pub requester: Option<ServiceAccountId>,
}
