use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use crate::{CredentialScope, PolicyFence, PullSecretId, ServiceAccountId};

/// A request for a short-lived token from the credential named by `pull_secret_id`.
///
/// `ttl_seconds` is never zero; deserialization rejects zero.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CredentialIssueRequest {
    pub pull_secret_id: PullSecretId,
    pub scope: CredentialScope,
    pub ttl_seconds: NonZeroU32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<ServiceAccountId>,
    /// Absent from clients that predate the fence. The broker accepts such a request unless it requires the fence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fence: Option<PolicyFence>,
}
