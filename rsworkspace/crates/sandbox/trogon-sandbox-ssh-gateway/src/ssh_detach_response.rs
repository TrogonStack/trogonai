use serde::{Deserialize, Serialize};

/// The worker's answer to a detach request.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshDetachResponse {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl SshDetachResponse {
    pub fn accepted() -> Self {
        Self {
            ok: true,
            message: None,
        }
    }

    pub fn rejected(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: Some(message.into()),
        }
    }
}
