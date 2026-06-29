//! ARD registry error response wire shape.

use serde::{Deserialize, Serialize};

/// ARD registry HTTP error envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryErrorResponseWire {
    pub error: RegistryErrorBodyWire,
}

/// ARD registry error payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryErrorBodyWire {
    pub code: String,
    pub message: String,
}

impl RegistryErrorResponseWire {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: RegistryErrorBodyWire {
                code: code.into(),
                message: message.into(),
            },
        }
    }
}
