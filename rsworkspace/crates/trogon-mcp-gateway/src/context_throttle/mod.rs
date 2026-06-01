//! Context-aware rate limits keyed by `(tenant_id, agent_id, purpose)`.
//!
//! **Gateway wiring:** After CEL evaluation in `gateway::handle_ingress_inner`
//! (`evaluate_hierarchical_policy` returns allow, ~line 373) and after the existing per-`jwt.sub`
//! caller check (`limiter.check_caller`, ~line 324), call [`ContextThrottle::acquire`] with
//! `tenant_id`, `agent_id`, and `purpose` from JWT / CEL Pin 8 variables. On
//! [`ContextThrottleOutcome::Throttled`], emit JSON-RPC using [`crate::rpc_codes::RATE_LIMITED`]
//! (`-32105`) with `data.scope = "purpose"` (same code as other rate-limit paths; do not add a
//! new RPC code). The legacy [`crate::throttle::ContextThrottler`] sliding-window path stays in
//! `throttle/` until Block E migrates it; this module is the token-bucket replacement keyed by
//! agent context rather than `jwt.sub` alone.

mod budget;
mod errors;
mod key;
mod state;
mod throttle;

use std::collections::HashMap;

pub use budget::ContextBudget;
pub use errors::ContextThrottleError;
pub use key::ContextThrottleKey;
pub use state::{Clock, SystemClock, TestClock};
pub use throttle::{ContextThrottle, ContextThrottleOutcome};

/// Configuration for per-context budgets (defaults plus per-`(tenant_id, agent_id)` overrides).
#[derive(Clone, Debug)]
pub struct ContextThrottleConfig {
    pub default_budget: ContextBudget,
    pub overrides: HashMap<(String, String), ContextBudget>,
}

impl Default for ContextThrottleConfig {
    fn default() -> Self {
        Self {
            default_budget: ContextBudget::from_pin9_defaults(),
            overrides: HashMap::new(),
        }
    }
}

impl ContextThrottleConfig {
    #[must_use]
    pub fn budget_for(&self, key: &ContextThrottleKey) -> ContextBudget {
        self.overrides
            .get(&(key.tenant_id.clone(), key.agent_id.clone()))
            .copied()
            .unwrap_or(self.default_budget)
    }
}
