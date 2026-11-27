use serde::Serialize;

use crate::ordinary_configuration::OrdinaryConfiguration;

/// One control request to the bridge.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum OrdinaryOperation {
    Init { config: OrdinaryConfiguration },
    Launch,
    Signal { signal: u32 },
    Stdin { data: String, eof: bool },
}

impl OrdinaryOperation {
    /// The name the bridge echoes in its acknowledgement.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Init { .. } => "init",
            Self::Launch => "launch",
            Self::Signal { .. } => "signal",
            Self::Stdin { .. } => "stdin",
        }
    }

    /// The signal the request carries, which the acknowledgement must echo.
    pub(crate) fn signal(&self) -> Option<u32> {
        match self {
            Self::Signal { signal } => Some(*signal),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
