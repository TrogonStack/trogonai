//! What the proxy does with an authenticated caller that holds no permit for
//! the destination it asked for. Enforcement is the default. Dry run is chosen
//! only by naming it, and it never relaxes authentication: it changes the one
//! answer "no permit" and nothing else.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EnforcementMode {
    #[default]
    Enforce,
    DryRun,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown enforcement mode, expected `enforce` or `dry_run`")]
pub struct EnforcementModeError;

impl EnforcementMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enforce => "enforce",
            Self::DryRun => "dry_run",
        }
    }
}

impl FromStr for EnforcementMode {
    type Err = EnforcementModeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "enforce" => Ok(Self::Enforce),
            "dry_run" => Ok(Self::DryRun),
            _ => Err(EnforcementModeError),
        }
    }
}

impl fmt::Display for EnforcementMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
