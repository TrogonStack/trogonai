//! Model switch orchestration: safety gate, continuity checkpoint, runner bindings, and state machines.

pub mod cancel;
pub mod checkpoint;
pub mod config;
pub mod error;
pub mod event;
pub mod force;
pub mod hydration;
pub mod nats;
pub mod orchestrator;
pub mod runner;
pub mod safety;
pub mod state;
pub mod telemetry;

pub use cancel::{
    CancelContext, CancelOutcome, RunnerCancelOutcome, RunnerCancellation, cancel_operation,
    tool_states_from_session,
};
pub use checkpoint::{
    ContinuityAcknowledgement, PassthroughCheckpointRunner, RunnerAcknowledgement,
    acknowledgement_from_context_twin, compare_acknowledgement, requires_continuity_checkpoint,
    risky_tools_blocked, run_continuity_checkpoint,
};
pub use config::SwitchingConfig;
pub use error::SwitchingError;
pub use event::SwitchFlowContext;
pub use force::{ForceSwitchOutcome, ForceSwitchRequest, evaluate_force_switch};
pub use hydration::{
    merge_portable_config, messages_json_for_runner_hydration, portable_config_from_snapshot,
    PortableRunnerConfig,
};
pub use orchestrator::{SwitchGateOutcome, SwitchModelRequest, SwitchOrchestrator, SwitchResult};
pub use runner::{
    RunnerBindingContext, RunnerBindingStore, attach_runner, detach_runner,
    invalidate_nonportable_runner_state, provision_runner_binding_store,
};
pub use safety::{SwitchSafetyInput, evaluate_switch_safety};
pub use state::{
    ArtifactPersistenceState, CancelState, ContinuityCheckpointState, ForceSwitchState,
    RunnerBindingState, SwitchState, ToolExecutionState,
};

#[cfg(any(test, feature = "test-support"))]
pub use checkpoint::mock::MockRunnerAcknowledgement;

#[cfg(any(test, feature = "test-support"))]
pub use cancel::mock::MockRunnerCancellation;
