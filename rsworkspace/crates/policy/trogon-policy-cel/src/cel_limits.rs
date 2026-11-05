use std::time::Duration;

use crate::constants::{DEFAULT_MAX_COLLECTION_LEN, DEFAULT_MAX_COST, DEFAULT_MAX_INPUT_BYTES, DEFAULT_TIMEOUT};

/// Why a set of limits cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CelLimitsError {
    /// A limit of zero would refuse every rule, including the ones that do
    /// nothing, and so would only ever be a typo.
    #[error("the {limit} limit must be greater than zero")]
    Zero {
        /// Which limit was zero.
        limit: &'static str,
    },
}

/// The bounds one rule evaluation runs inside.
///
/// The cost limit is checked when a rule is compiled, against a static upper
/// bound on the work it can do over facts that satisfy the input limits, so a
/// rule that could exceed it never loads. The timeout is checked at every rule
/// boundary against an injected clock. The interpreter offers no way to
/// interrupt a running expression, so the timeout cannot stop work already
/// started; what keeps one rule from running long is the cost limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CelLimits {
    max_cost: u64,
    timeout: Duration,
    max_input_bytes: usize,
    max_collection_len: usize,
}

impl CelLimits {
    /// Sets the cost limit and the timeout, and takes the defaults for the
    /// bounds on the facts.
    pub fn new(max_cost: u64, timeout: Duration) -> Result<Self, CelLimitsError> {
        if max_cost == 0 {
            return Err(CelLimitsError::Zero { limit: "cost" });
        }
        if timeout.is_zero() {
            return Err(CelLimitsError::Zero { limit: "timeout" });
        }
        Ok(Self {
            max_cost,
            timeout,
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
            max_collection_len: DEFAULT_MAX_COLLECTION_LEN,
        })
    }

    /// Bounds the size of the facts one evaluation reads.
    pub fn with_max_input_bytes(self, max_input_bytes: usize) -> Result<Self, CelLimitsError> {
        if max_input_bytes == 0 {
            return Err(CelLimitsError::Zero { limit: "input size" });
        }
        Ok(Self {
            max_input_bytes,
            ..self
        })
    }

    /// Bounds the length of every list and map in the facts. The static cost
    /// of a rule that loops over a fact is computed with this length.
    pub fn with_max_collection_len(self, max_collection_len: usize) -> Result<Self, CelLimitsError> {
        if max_collection_len == 0 {
            return Err(CelLimitsError::Zero {
                limit: "collection length",
            });
        }
        Ok(Self {
            max_collection_len,
            ..self
        })
    }

    /// The most static cost a rule may have.
    pub const fn max_cost(&self) -> u64 {
        self.max_cost
    }

    /// The wall-clock time one evaluation may take.
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// The most bytes of facts one evaluation reads.
    pub const fn max_input_bytes(&self) -> usize {
        self.max_input_bytes
    }

    /// The longest list or map a fact may hold.
    pub const fn max_collection_len(&self) -> usize {
        self.max_collection_len
    }
}

impl Default for CelLimits {
    fn default() -> Self {
        Self {
            max_cost: DEFAULT_MAX_COST,
            timeout: DEFAULT_TIMEOUT,
            max_input_bytes: DEFAULT_MAX_INPUT_BYTES,
            max_collection_len: DEFAULT_MAX_COLLECTION_LEN,
        }
    }
}
