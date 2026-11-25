use std::time::Duration;

use crate::constants::MAX_STALENESS_BUDGET;

/// Why a duration cannot be a staleness budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StalenessBudgetError {
    /// A zero budget would refuse every snapshot that is not stamped at the
    /// instant of evaluation.
    #[error("staleness budget cannot be zero")]
    Zero,
    /// ADR#0071 Decision 7: an enforcement point never sets its bound above the
    /// hard ceiling of ADR#0049.
    #[error("staleness budget {requested:?} is above the ceiling of {ceiling:?}")]
    AboveCeiling {
        /// What was asked for.
        requested: Duration,
        /// The hard ceiling.
        ceiling: Duration,
    },
}

/// How old a snapshot may be before the evaluator refuses to use it.
///
/// Defaults to the hard ceiling of 330 seconds (the 300 second cache TTL of
/// ADR#0049 plus up to 30 seconds of jitter). An enforcement point may tighten
/// it and may never loosen it. The 5 second p99 target of ADR#0049 is something
/// to alert on, not something to refuse at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StalenessBudget(Duration);

impl StalenessBudget {
    /// Creates a budget no longer than the ceiling.
    pub fn new(budget: Duration) -> Result<Self, StalenessBudgetError> {
        if budget.is_zero() {
            return Err(StalenessBudgetError::Zero);
        }
        if budget > MAX_STALENESS_BUDGET {
            return Err(StalenessBudgetError::AboveCeiling {
                requested: budget,
                ceiling: MAX_STALENESS_BUDGET,
            });
        }
        Ok(Self(budget))
    }

    /// The longest budget that is allowed.
    pub const fn ceiling() -> Self {
        Self(MAX_STALENESS_BUDGET)
    }

    /// The budget as a duration.
    pub const fn get(self) -> Duration {
        self.0
    }
}

impl Default for StalenessBudget {
    fn default() -> Self {
        Self::ceiling()
    }
}
