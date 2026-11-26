use serde::{Deserialize, Serialize};

/// Whether a reap releases the leases it matches.
///
/// It serializes as the JSON boolean `dry_run`, `true` for `DryRun`, so existing peers keep parsing it.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum ReapMode {
    /// Nothing is released. The response only reports what would be.
    DryRun,
    /// Matching leases are released.
    #[default]
    Release,
}

impl ReapMode {
    pub const fn is_dry_run(self) -> bool {
        matches!(self, Self::DryRun)
    }
}

impl From<bool> for ReapMode {
    fn from(dry_run: bool) -> Self {
        if dry_run { Self::DryRun } else { Self::Release }
    }
}

impl From<ReapMode> for bool {
    fn from(mode: ReapMode) -> Self {
        mode.is_dry_run()
    }
}
