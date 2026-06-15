use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RRuleWakeupPayload {
    schedule_id: String,
    occurrence_at: String,
}

impl RRuleWakeupPayload {
    pub(crate) fn new(schedule_id: impl Into<String>, occurrence_at: DateTime<Utc>) -> Self {
        Self {
            schedule_id: schedule_id.into(),
            occurrence_at: occurrence_at.to_rfc3339_opts(SecondsFormat::AutoSi, true),
        }
    }

    pub(crate) fn schedule_id(&self) -> &str {
        &self.schedule_id
    }

    pub(crate) fn occurrence_at(&self) -> Result<DateTime<Utc>, chrono::ParseError> {
        DateTime::parse_from_rfc3339(&self.occurrence_at).map(|datetime| datetime.with_timezone(&Utc))
    }

    pub(crate) fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("rrule wakeup payload is JSON-serializable")
    }

    pub(crate) fn decode(payload: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(payload)
    }
}
