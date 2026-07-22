use trogon_decider_runtime::StreamPosition;

use crate::commands::domain::{Delivery, Schedule, ScheduleId, ScheduleMessage};
use crate::processor::execution::reconciliation::{ScheduleKey, ScheduleSubject};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleStatus {
    Scheduled,
    Paused,
    Removed,
    Unsupported,
    Expired,
    /// A status persisted by a newer deployment that this version does not
    /// recognize. Kept readable so a rolling deploy cannot poison the
    /// schedule; the next reconciled event overwrites it with a known status.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileOutcome {
    Published,
    Purged,
    StoredPaused,
    Unsupported,
    Expired,
    DuplicateStale,
    /// An outcome persisted by a newer deployment that this version does not
    /// recognize. Informational only; never written by this version.
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleCheckpointRecord {
    pub schedule_id: ScheduleId,
    pub status: ScheduleStatus,
    pub schedule: Schedule,
    pub delivery: Delivery,
    pub message: ScheduleMessage,
    pub last_applied_stream_position: StreamPosition,
    pub last_applied_event_id: Option<String>,
    pub last_outcome: ReconcileOutcome,
}

impl ScheduleCheckpointRecord {
    /// KV key for this schedule, derived deterministically from the id so it
    /// can never drift from what was stored.
    pub fn key(&self) -> ScheduleKey {
        ScheduleKey::derive(&self.schedule_id)
    }

    /// Execution stream subject for this schedule, derived deterministically
    /// from the id so it can never drift from what was stored.
    pub fn subject(&self) -> ScheduleSubject {
        ScheduleSubject::execution(&self.key())
    }
}
