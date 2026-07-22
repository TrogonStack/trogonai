//! ARD registry federation mode.

use serde::{Deserialize, Serialize};

/// ARD search federation mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FederationMode {
    #[default]
    None,
    Referrals,
    Auto,
}

impl FederationMode {
    pub fn includes_referrals(self) -> bool {
        matches!(self, Self::Referrals | Self::Auto)
    }
}

#[cfg(test)]
mod tests;
