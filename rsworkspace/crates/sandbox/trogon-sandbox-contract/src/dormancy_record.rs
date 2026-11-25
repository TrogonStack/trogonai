use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{DormancyArtifact, SuspendLevel};

/// What the broker records about a lease's dormancy. It is set when a suspend
/// starts and kept after the wake (with `woken_at` set), so the generation
/// keeps counting up across suspend and wake cycles.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DormancyRecord {
    pub level: SuspendLevel,
    /// Counts suspends of this lease. A worker request carries it so a retry
    /// reuses the work the first attempt did instead of repeating it.
    pub generation: u64,
    /// `None` while the provider keeps the state under its own sandbox id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<DormancyArtifact>,
    /// Bytes the artifact takes, when the worker reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_bytes: Option<u64>,
    pub suspended_at: SystemTime,
    /// Seconds of run time the lease had left when the suspend started,
    /// restored as the deadline on wake.
    pub run_remaining_seconds: u64,
    /// Wakes in a row that failed. The broker marks the lease failed after three.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub wake_failures: u32,
    /// When the lease last woke. `Some` once the dormancy is over.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub woken_at: Option<SystemTime>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}
