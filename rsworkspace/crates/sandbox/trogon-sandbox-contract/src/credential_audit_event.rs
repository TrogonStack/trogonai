use std::num::NonZeroU32;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{CredentialId, CredentialScope, ServiceAccountId, TenantId, TokenId};

/// A record of a credential being issued, revoked, or rotated, tagged by `event`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum CredentialAuditEvent {
    Issued {
        tenant: TenantId,
        credential_id: CredentialId,
        token_id: TokenId,
        scope: CredentialScope,
        ttl_seconds: NonZeroU32,
        requester: ServiceAccountId,
        at: SystemTime,
    },
    Revoked {
        token_id: TokenId,
        reason: String,
        at: SystemTime,
    },
    Rotated {
        credential_id: CredentialId,
        new_version: u32,
        at: SystemTime,
    },
}
