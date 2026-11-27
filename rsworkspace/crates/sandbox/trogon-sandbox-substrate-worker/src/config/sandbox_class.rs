use std::fmt;
use std::str::FromStr;

use trogon_sandbox_contract::IsolationTier;

use super::SandboxClassError;

/// The runtime family an actor runs on.
///
/// Snapshots are not portable across classes, so this is part of the template key and a
/// hard scheduling gate rather than a hint.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum SandboxClass {
    Gvisor,
    MicroVm,
}

impl SandboxClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gvisor => "gvisor",
            Self::MicroVm => "microvm",
        }
    }

    /// The isolation tier this class satisfies, as the rest of the stack names it.
    pub fn isolation_tier(self) -> IsolationTier {
        match self {
            Self::Gvisor => IsolationTier::UserSpaceKernel,
            Self::MicroVm => IsolationTier::MicroVm,
        }
    }

    /// The class that satisfies a tier a lease asked for, if this worker can serve it.
    pub fn for_tier(tier: IsolationTier) -> Option<Self> {
        match tier {
            IsolationTier::UserSpaceKernel => Some(Self::Gvisor),
            IsolationTier::MicroVm => Some(Self::MicroVm),
            _ => None,
        }
    }
}

impl fmt::Display for SandboxClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SandboxClass {
    type Err = SandboxClassError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "gvisor" | "user_space_kernel" | "userspacekernel" => Ok(Self::Gvisor),
            "microvm" | "micro_vm" | "micro-vm" => Ok(Self::MicroVm),
            other => Err(SandboxClassError::new(other)),
        }
    }
}
