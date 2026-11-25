use serde::{Deserialize, Serialize};

/// Whether a sandbox may write to a mounted drive. An omitted mode reads as `ReadWrite`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveMountMode {
    #[default]
    ReadWrite,
    ReadOnly,
}
