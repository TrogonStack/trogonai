use serde::{Deserialize, Serialize};

use crate::{CredentialAck, CredentialError};

/// The credential broker's reply to a request that returns no token, tagged by `outcome`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CredentialAckOutcome {
    Ok(CredentialAck),
    Err(CredentialError),
}
