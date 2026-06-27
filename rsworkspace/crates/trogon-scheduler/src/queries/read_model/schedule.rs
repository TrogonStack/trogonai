use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{MessageEnvelope, ScheduleDetails, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Schedule {
    pub id: String,
    #[serde(default, rename = "state")]
    pub status: ScheduleEventStatus,
    /// A recurring schedule that has run to exhaustion is terminal: it stays
    /// visible in the read model but will never fire again. Tracked separately
    /// from `status` (mirroring the command aggregate's `completed` flag) so a
    /// completed schedule is not indistinguishable from an active one.
    #[serde(default)]
    pub completed: bool,
    /// The next planned occurrence, if one is armed and pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_occurrence_at: Option<DateTime<Utc>>,
    /// The most recently recorded occurrence, if any has fired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_occurrence_at: Option<DateTime<Utc>>,
    pub schedule: ScheduleEventSchedule,
    pub delivery: ScheduleEventDelivery,
    pub message: MessageEnvelope,
}

impl Schedule {
    pub fn is_enabled(&self) -> bool {
        !self.completed && matches!(self.status, ScheduleEventStatus::Scheduled)
    }
}

impl From<(String, ScheduleDetails)> for Schedule {
    fn from((id, details): (String, ScheduleDetails)) -> Self {
        Self {
            id,
            status: details.status,
            completed: false,
            next_occurrence_at: None,
            last_occurrence_at: None,
            schedule: details.schedule,
            delivery: details.delivery,
            message: details.message,
        }
    }
}

#[cfg(test)]
mod tests;
