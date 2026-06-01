//! Step-up authentication signaling for sensitive tools.
//!
//! **Gateway wiring (future PR):** After CEL evaluation in `gateway::handle_ingress_inner`
//! (post-`evaluate_hierarchical_policy`, same hook documented in `approvals/mod.rs`), when
//! policy allows the call but the tool carries `sensitive: true` (and any CEL step-up rule),
//! call [`StepUpPolicy::evaluate`]. On [`StepUpDemand::Reauth`], mint JSON-RPC using
//! [`StepUpOutcome::jsonrpc_code`]; on [`StepUpDemand::Approval`], call [`ApprovalBridge::escalate`]
//! then emit `-32107 approval_required` via `approvals::build_approval_required` (or the
//! step-up subject variant when the wire contract is pinned for humans).

mod bridge;
mod errors;
mod freshness;
mod policy;

pub use bridge::{ApprovalBridge, NoopApprovalBridge};
pub use errors::StepUpError;
pub use freshness::{FreshnessClock, SystemFreshnessClock, TestFreshnessClock, is_auth_time_fresh};
pub use policy::{
    StepUpDemand, StepUpPolicy, StepUpRequestCtx, ToolAnnotations, DEFAULT_MAX_AUTH_AGE_SECS,
};

use crate::rpc_codes;

/// Resolved gateway action after step-up policy evaluation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepUpOutcome {
    Allow,
    Reauth { max_age_seconds: u64 },
    Approval { reason: String },
    Error(StepUpError),
}

impl StepUpOutcome {
    #[must_use]
    pub fn from_demand(demand: StepUpDemand) -> Self {
        match demand {
            StepUpDemand::None => Self::Allow,
            StepUpDemand::Reauth { max_age_seconds } => Self::Reauth { max_age_seconds },
            StepUpDemand::Approval { reason } => Self::Approval { reason },
        }
    }

    /// JSON-RPC `error.code` the ingress layer should emit when this outcome blocks the call.
    #[must_use]
    pub fn jsonrpc_code(&self) -> Option<i32> {
        match self {
            Self::Allow | Self::Error(_) => None,
            Self::Reauth { .. } => Some(rpc_codes::AUTH_EXPIRED),
            Self::Approval { .. } => Some(rpc_codes::APPROVAL_REQUIRED),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stepup::freshness::FreshnessClock;

    struct FixedClock(i64);

    impl FreshnessClock for FixedClock {
        fn now_unix(&self) -> i64 {
            self.0
        }
    }

    #[test]
    fn outcome_maps_reauth_to_auth_expired_code() {
        let outcome = StepUpOutcome::from_demand(StepUpDemand::Reauth {
            max_age_seconds: 300,
        });
        assert_eq!(outcome.jsonrpc_code(), Some(rpc_codes::AUTH_EXPIRED));
    }

    #[test]
    fn outcome_maps_approval_to_approval_required_code() {
        let outcome = StepUpOutcome::from_demand(StepUpDemand::Approval {
            reason: "test".into(),
        });
        assert_eq!(outcome.jsonrpc_code(), Some(rpc_codes::APPROVAL_REQUIRED));
    }

    #[test]
    fn policy_evaluate_through_outcome() {
        let policy = StepUpPolicy::default();
        let clock = FixedClock(2_000);
        let demand = policy
            .evaluate(
                &clock,
                &StepUpRequestCtx {
                    request_id: "r".into(),
                    auth_method: Some("password".into()),
                    auth_time: Some(1_000),
                },
                &ToolAnnotations { sensitive: true },
            )
            .expect("evaluate");
        let outcome = StepUpOutcome::from_demand(demand);
        assert_eq!(
            outcome,
            StepUpOutcome::Reauth {
                max_age_seconds: DEFAULT_MAX_AUTH_AGE_SECS,
            }
        );
    }
}
