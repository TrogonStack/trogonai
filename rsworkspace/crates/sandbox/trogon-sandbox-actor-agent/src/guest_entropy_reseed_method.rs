use serde::{Deserialize, Serialize};

/// How the guest mixed a seed into its kernel.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestEntropyReseedMethod {
    LinuxKernelV1,
}
