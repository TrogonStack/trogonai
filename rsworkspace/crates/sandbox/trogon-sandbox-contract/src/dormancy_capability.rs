use serde::{Deserialize, Serialize};

use crate::{CapabilitySupport, SuspendLevel, SuspendRequestLevel};

/// The dormancy forms a worker offers.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DormancyCapability {
    /// Freezes the lease in place: processes and memory stay resident and host resources stay held.
    #[serde(default)]
    pub freeze: CapabilitySupport,
    /// The deepest suspend offered, which releases host resources.
    #[serde(default)]
    pub suspend: Option<SuspendLevel>,
}

impl DormancyCapability {
    pub const NONE: Self = Self {
        freeze: CapabilitySupport::Unsupported,
        suspend: None,
    };

    pub const FREEZE: Self = Self {
        freeze: CapabilitySupport::Supported,
        suspend: None,
    };

    pub const SUSPEND_DISK: Self = Self {
        freeze: CapabilitySupport::Unsupported,
        suspend: Some(SuspendLevel::Disk),
    };

    pub const SUSPEND_MEMORY: Self = Self {
        freeze: CapabilitySupport::Unsupported,
        suspend: Some(SuspendLevel::Memory),
    };

    /// Returns the level a suspend request lands on, or `None` when it cannot be honoured.
    ///
    /// A request for the level the worker does not offer is refused rather than downgraded or
    /// upgraded, because the levels promise different things about processes.
    pub fn resolve(self, requested: SuspendRequestLevel) -> Option<SuspendLevel> {
        let offered = self.suspend?;
        match (requested, offered) {
            (SuspendRequestLevel::Auto, level)
            | (SuspendRequestLevel::Memory, level @ SuspendLevel::Memory)
            | (SuspendRequestLevel::Disk, level @ SuspendLevel::Disk) => Some(level),
            _ => None,
        }
    }
}
