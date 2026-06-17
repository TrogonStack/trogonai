use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::commands::domain::{ScheduleId, ScheduleIdError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RRuleWakeupPayloadWire {
    schedule_id: String,
    occurrence_at: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RRuleWakeupPayloadDecodeError {
    #[error("RRULE wakeup payload is invalid JSON: {source}")]
    Json {
        #[source]
        source: serde_json::Error,
    },
    #[error("RRULE wakeup schedule id is invalid: {source}")]
    ScheduleId {
        #[source]
        source: ScheduleIdError,
    },
    #[error("RRULE wakeup occurrence_at is invalid: {source}")]
    OccurrenceAt {
        #[source]
        source: chrono::ParseError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RRuleWakeupPayload {
    schedule_id: ScheduleId,
    occurrence_at: DateTime<Utc>,
}

impl RRuleWakeupPayload {
    pub(crate) fn new(schedule_id: ScheduleId, occurrence_at: DateTime<Utc>) -> Self {
        Self {
            schedule_id,
            occurrence_at,
        }
    }

    pub(crate) fn schedule_id(&self) -> &ScheduleId {
        &self.schedule_id
    }

    pub(crate) fn occurrence_at(&self) -> DateTime<Utc> {
        self.occurrence_at
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        let wire = RRuleWakeupPayloadWire {
            schedule_id: self.schedule_id.as_str().to_string(),
            occurrence_at: self.occurrence_at.to_rfc3339_opts(SecondsFormat::AutoSi, true),
        };
        serde_json::to_vec(&wire).expect("rrule wakeup payload is JSON-serializable")
    }

    pub(crate) fn decode(payload: &[u8]) -> Result<Self, RRuleWakeupPayloadDecodeError> {
        let wire: RRuleWakeupPayloadWire =
            serde_json::from_slice(payload).map_err(|source| RRuleWakeupPayloadDecodeError::Json { source })?;
        wire.try_into()
    }
}

impl TryFrom<RRuleWakeupPayloadWire> for RRuleWakeupPayload {
    type Error = RRuleWakeupPayloadDecodeError;

    fn try_from(wire: RRuleWakeupPayloadWire) -> Result<Self, Self::Error> {
        let schedule_id = ScheduleId::parse(&wire.schedule_id)
            .map_err(|source| RRuleWakeupPayloadDecodeError::ScheduleId { source })?;
        let occurrence_at = DateTime::parse_from_rfc3339(&wire.occurrence_at)
            .map(|datetime| datetime.with_timezone(&Utc))
            .map_err(|source| RRuleWakeupPayloadDecodeError::OccurrenceAt { source })?;
        Ok(Self {
            schedule_id,
            occurrence_at,
        })
    }
}
