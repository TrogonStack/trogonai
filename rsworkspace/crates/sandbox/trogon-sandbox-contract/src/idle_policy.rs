use serde::{Deserialize, Serialize};

use crate::{IdlePolicyError, IdleStage};

/// Staged idle handling, for example pause after five minutes of silence and release after an hour.
///
/// Each stage fires once, activity restarts the chain, and stages only get more severe.
/// Deserialization does not validate; call [`IdlePolicy::validate`] with the operator's limit.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct IdlePolicy {
    pub stages: Vec<IdleStage>,
}

impl IdlePolicy {
    /// Rejects a policy the evaluator could not run unambiguously.
    ///
    /// The policy needs at least one stage, every stage time within `1..=max_seconds`, strictly
    /// increasing stage times, and strictly more severe actions, so a pause after a release and a
    /// repeated action are refused.
    pub fn validate(&self, max_seconds: u32) -> Result<(), IdlePolicyError> {
        if self.stages.is_empty() {
            return Err(IdlePolicyError::NoStages);
        }
        if self
            .stages
            .iter()
            .any(|stage| stage.after_seconds == 0 || stage.after_seconds > max_seconds)
        {
            return Err(IdlePolicyError::StageOutOfRange { max_seconds });
        }
        for (previous, next) in self.stages.iter().zip(self.stages.iter().skip(1)) {
            if next.after_seconds <= previous.after_seconds {
                return Err(IdlePolicyError::UnorderedStages);
            }
            if next.action <= previous.action {
                return Err(IdlePolicyError::NonEscalatingActions);
            }
        }
        Ok(())
    }
}
