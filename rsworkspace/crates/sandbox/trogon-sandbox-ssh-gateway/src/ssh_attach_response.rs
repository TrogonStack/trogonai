use serde::{Deserialize, Serialize};

use crate::SshAttachRefusal;

/// The worker's answer to an attach request.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshAttachResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Why a refused attach was refused, when the worker can say. Absent for an accepted attach and for a failure the
    /// worker did not classify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<SshAttachRefusal>,
}

impl SshAttachResponse {
    pub fn accepted() -> Self {
        Self {
            ok: true,
            message: None,
            refusal: None,
        }
    }

    pub fn rejected(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: Some(message.into()),
            refusal: None,
        }
    }

    /// The worker holds the lease but its provider cannot serve SSH through the gateway, so retrying will not help.
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: Some(message.into()),
            refusal: Some(SshAttachRefusal::Unsupported),
        }
    }
}
