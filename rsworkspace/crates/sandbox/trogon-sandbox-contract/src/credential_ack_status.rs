use serde::{Deserialize, Serialize};

/// Whether the credential broker accepted a request, as carried by a credential ack's `ok` field.
///
/// It serializes as a JSON boolean, `true` for `Accepted`, so existing peers keep parsing it.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum CredentialAckStatus {
    Accepted,
    Refused,
}

impl From<bool> for CredentialAckStatus {
    fn from(ok: bool) -> Self {
        if ok { Self::Accepted } else { Self::Refused }
    }
}

impl From<CredentialAckStatus> for bool {
    fn from(status: CredentialAckStatus) -> Self {
        matches!(status, CredentialAckStatus::Accepted)
    }
}
