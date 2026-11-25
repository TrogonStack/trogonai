use serde::{Deserialize, Serialize};

/// How deeply a worker can put a lease to sleep.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuspendLevel {
    /// Memory and processes are saved, so they continue after wake.
    Memory,
    /// Only the disk survives; processes end.
    Disk,
}

impl SuspendLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Disk => "disk",
        }
    }
}
