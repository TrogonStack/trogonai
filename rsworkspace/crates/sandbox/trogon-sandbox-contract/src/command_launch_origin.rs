use serde::{Deserialize, Serialize};

/// Whether the worker created an execution for a logical launch or joined existing work.
///
/// `Unknown` is the default and is left off the wire.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandLaunchOrigin {
    Created,
    Reused,
    #[default]
    Unknown,
}

impl CommandLaunchOrigin {
    pub fn is_unknown(&self) -> bool {
        *self == Self::Unknown
    }
}
