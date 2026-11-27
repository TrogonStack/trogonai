use serde::{Deserialize, Serialize};

use crate::{CredentialProvider, CredentialScope, PullSecretId, ServiceAccountId};

/// Which credential to fetch from the broker for a [`CredentialTransform`](crate::CredentialTransform).
///
/// It mirrors the fields a caller supplies when it asks the broker to issue a credential.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CredentialRequest {
    pub provider: CredentialProvider,
    pub pull_secret_id: PullSecretId,
    pub scope: CredentialScope,
    pub ttl_seconds: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<ServiceAccountId>,
}
