use crate::VmIdError;
use crate::constants::{MAX_VM_ID, MIN_VM_ID};

/// A Proxmox virtual machine id, inside the range Proxmox allocates from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct VmId(u32);

impl TryFrom<u32> for VmId {
    type Error = VmIdError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if (MIN_VM_ID..=MAX_VM_ID).contains(&value) {
            Ok(Self(value))
        } else {
            Err(VmIdError::OutOfRange(value))
        }
    }
}

impl From<VmId> for u32 {
    fn from(value: VmId) -> Self {
        value.0
    }
}

impl std::str::FromStr for VmId {
    type Err = VmIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(VmIdError::NotDecimal);
        }
        Self::try_from(value.parse::<u32>().map_err(|_| VmIdError::Overflow)?)
    }
}

impl std::fmt::Display for VmId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
mod tests;
