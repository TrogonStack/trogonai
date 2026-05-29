/// Token-bucket budget for a `(tenant_id, agent_id, purpose)` tuple.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextBudget {
    pub tokens_per_minute: u32,
    pub burst: u32,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self::from_pin9_defaults()
    }
}

impl ContextBudget {
    /// Wire-Format Pin 9 adaptive-access purpose throttle defaults (60/min, burst 30).
    #[must_use]
    pub const fn from_pin9_defaults() -> Self {
        Self {
            tokens_per_minute: 60,
            burst: 30,
        }
    }

    #[must_use]
    pub const fn capacity(&self) -> f64 {
        self.burst as f64
    }

    #[must_use]
    pub fn refill_per_sec(&self) -> f64 {
        self.tokens_per_minute as f64 / 60.0
    }
}
