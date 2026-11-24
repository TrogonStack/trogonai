use serde::{Deserialize, Serialize};

use crate::CredentialAckStatus;

/// The JSON shape of a [`CredentialAck`](crate::CredentialAck) before its `ok` status is checked.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CredentialAckWire {
    pub(crate) ok: CredentialAckStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
}
