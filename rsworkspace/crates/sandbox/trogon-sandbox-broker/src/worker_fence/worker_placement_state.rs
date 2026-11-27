use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerPlacementState {
    #[default]
    Active,
    Cordoned,
    Draining,
}

impl WorkerPlacementState {
    /// Whether placement may choose a worker in this state for a new lease.
    pub fn admits_placements(self) -> bool {
        matches!(self, Self::Active)
    }
}

impl std::fmt::Display for WorkerPlacementState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Active => "active",
            Self::Cordoned => "cordoned",
            Self::Draining => "draining",
        })
    }
}
