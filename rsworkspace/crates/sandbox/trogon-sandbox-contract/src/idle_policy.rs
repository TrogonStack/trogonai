use serde::{Deserialize, Deserializer, Serialize, de};

use crate::idle_policy_wire::IdlePolicyWire;
use crate::{IdlePolicyError, IdleStage};

/// Staged idle handling, for example pause after five minutes of silence and release after an hour.
///
/// Each stage fires once and activity restarts the chain. A policy holds at least one stage, its
/// stage times strictly increase, and its actions get strictly more severe, so a pause after a
/// release or a repeated action cannot be expressed. Construction and deserialization both enforce
/// it.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize)]
pub struct IdlePolicy {
    stages: Vec<IdleStage>,
}

impl IdlePolicy {
    pub fn new(stages: Vec<IdleStage>) -> Result<Self, IdlePolicyError> {
        if stages.is_empty() {
            return Err(IdlePolicyError::NoStages);
        }
        for (previous, next) in stages.iter().zip(stages.iter().skip(1)) {
            if next.after_seconds <= previous.after_seconds {
                return Err(IdlePolicyError::UnorderedStages);
            }
            if next.action <= previous.action {
                return Err(IdlePolicyError::NonEscalatingActions);
            }
        }
        Ok(Self { stages })
    }

    pub fn stages(&self) -> &[IdleStage] {
        &self.stages
    }

    /// Reports whether every stage fires within `max_seconds`, an operator's ceiling on idle time.
    pub fn fits_within(&self, max_seconds: u32) -> bool {
        self.stages.iter().all(|stage| stage.after_seconds.get() <= max_seconds)
    }
}

impl<'de> Deserialize<'de> for IdlePolicy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = IdlePolicyWire::deserialize(deserializer)?;
        Self::new(wire.stages).map_err(de::Error::custom)
    }
}
