//! Wire identifiers for the Act chain fixture decider.

/// Event type name for [`PlanEvent::StepOneApplied`](crate::PlanEvent::StepOneApplied).
pub const STEP_ONE_EVENT_TYPE: &str = "trogon.decider.wasm_runtime.fixtures.act_chain.v1.StepOneApplied";
/// Event type name for [`PlanEvent::StepTwoApplied`](crate::PlanEvent::StepTwoApplied).
pub const STEP_TWO_EVENT_TYPE: &str = "trogon.decider.wasm_runtime.fixtures.act_chain.v1.StepTwoApplied";
/// Wire type URL for [`RunTwoStepPlan`](crate::RunTwoStepPlan).
pub const RUN_TWO_STEP_PLAN_TYPE_URL: &str =
    "type.googleapis.com/trogon.decider.wasm_runtime.fixtures.act_chain.v1.RunTwoStepPlan";
/// Snapshot schema version for [`PlanState`](crate::PlanState).
pub const PLAN_STATE_SCHEMA_VERSION: &str = "wasm_runtime_fixtures.act_chain.v1.PlanState";
