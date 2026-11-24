use serde::{Deserialize, Serialize};

/// The JSON shape of a [`CredentialAck`](crate::CredentialAck) before its `ok` flag is checked.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CredentialAckWire {
    pub(crate) ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
}
