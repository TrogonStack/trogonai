use std::fmt;

use serde::{Deserialize, Serialize};

/// Where a credential for a workspace clone comes from.
///
/// `Debug` and `Display` never show a literal value, so logging a reference cannot leak the secret.
/// Serialization still carries it, because the receiving worker needs it.
#[derive(Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretRef {
    /// A value stored under `key` in a NATS key-value `bucket`.
    NatsKv { bucket: String, key: String },
    /// The secret itself, inline.
    Literal { value: String },
}

impl fmt::Debug for SecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NatsKv { bucket, key } => formatter
                .debug_struct("NatsKv")
                .field("bucket", bucket)
                .field("key", key)
                .finish(),
            Self::Literal { .. } => formatter.debug_struct("Literal").field("value", &"[REDACTED]").finish(),
        }
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NatsKv { bucket, key } => write!(formatter, "nats-kv:{bucket}/{key}"),
            Self::Literal { .. } => formatter.write_str("[REDACTED]"),
        }
    }
}
