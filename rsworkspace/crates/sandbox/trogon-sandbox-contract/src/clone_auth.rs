use serde::{Deserialize, Serialize};

use crate::{EmptyObject, SecretRef};

/// How a worker authenticates a workspace clone. Secrets are [`SecretRef`] values, so `Debug`
/// never shows a literal one.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloneAuth {
    /// An anonymous clone, serialized as `{"none":{}}`.
    None(EmptyObject),
    BearerToken {
        token_ref: SecretRef,
    },
    Basic {
        username: String,
        password_ref: SecretRef,
    },
}
