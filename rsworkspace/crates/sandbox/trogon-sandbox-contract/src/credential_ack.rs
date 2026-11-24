use serde::{Deserialize, Serialize};

use crate::credential_ack_wire::CredentialAckWire;
use crate::{CredentialAckError, CredentialAckStatus};

/// The credential broker's acknowledgement of a request that returns no token.
///
/// It serializes with `ok: true`. Deserialization rejects `ok: false`, because the broker reports a failure as a
/// [`CredentialError`](crate::CredentialError) instead.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CredentialAckWire", into = "CredentialAckWire")]
pub struct CredentialAck {
    pub message: Option<String>,
}

impl TryFrom<CredentialAckWire> for CredentialAck {
    type Error = CredentialAckError;

    fn try_from(wire: CredentialAckWire) -> Result<Self, Self::Error> {
        match wire.ok {
            CredentialAckStatus::Accepted => Ok(Self { message: wire.message }),
            CredentialAckStatus::Refused => Err(CredentialAckError::NotOk),
        }
    }
}

impl From<CredentialAck> for CredentialAckWire {
    fn from(ack: CredentialAck) -> Self {
        Self {
            ok: CredentialAckStatus::Accepted,
            message: ack.message,
        }
    }
}
