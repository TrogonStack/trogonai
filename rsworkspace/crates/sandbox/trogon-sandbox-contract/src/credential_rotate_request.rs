use serde::{Deserialize, Serialize};

use crate::CredentialId;

/// A request to rotate a stored credential.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CredentialRotateRequest {
    pub credential_id: CredentialId,
}
