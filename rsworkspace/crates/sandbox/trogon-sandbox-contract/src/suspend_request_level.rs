use serde::{Deserialize, Serialize};

/// The suspend level a caller asks for. `Auto` takes whatever the lease's provider offers.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuspendRequestLevel {
    #[default]
    Auto,
    Memory,
    Disk,
}

impl SuspendRequestLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Memory => "memory",
            Self::Disk => "disk",
        }
    }
}
