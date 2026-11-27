use std::str::FromStr;

use crate::FenceModeError;

/// How a request without a policy fence is treated.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum FenceMode {
    /// Accept a request without a fence (clients that predate it); verify one that has it.
    #[default]
    Optional,
    /// Refuse a request without a fence.
    Required,
}

impl FromStr for FenceMode {
    type Err = FenceModeError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "optional" => Ok(Self::Optional),
            "required" => Ok(Self::Required),
            _ => Err(FenceModeError),
        }
    }
}
