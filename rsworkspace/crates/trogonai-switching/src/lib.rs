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
pub mod visible_result;

pub use cancel::{
    CancelContext, CancelOutcome, RunnerCancelOutcome, RunnerCancellation, cancel_operation, tool_states_from_session,
};
pub use checkpoint::{
    AckTransport, ContinuityAcknowledgement, JsonAcknowledgementRunner, PassthroughCheckpointRunner,
    RunnerAcknowledgement, acknowledgement_from_context_twin, compare_acknowledgement, frame_acknowledgement_request,
    parse_acknowledgement, requires_continuity_checkpoint, risky_tools_blocked, run_continuity_checkpoint,
};
pub use config::{DegradationPolicy, SwitchingConfig};
pub use error::SwitchingError;
pub use event::SwitchFlowContext;
pub use force::{ForceSwitchOutcome, ForceSwitchRequest, evaluate_force_switch};
pub use hydration::{
    PortableRunnerConfig, merge_portable_config, messages_json_for_runner_hydration, portable_config_from_snapshot,
};
pub use orchestrator::{
    SwitchCompletion, SwitchGateOutcome, SwitchModelRequest, SwitchOrchestrator, classify_switch_result,
};
pub use runner::{
    RunnerBindingContext, RunnerBindingStore, attach_runner, detach_runner, invalidate_nonportable_runner_state,
    provision_runner_binding_store,
};
pub use safety::{SwitchSafetyInput, evaluate_switch_safety};
pub use visible_result::{VisibleResultContext, build_visible_result};
pub use state::{
    ArtifactPersistenceState, CancelState, ContinuityCheckpointState, ForceSwitchState, RunnerBindingState,
    SwitchState, ToolExecutionState,
};

#[cfg(any(test, feature = "test-support"))]
pub use checkpoint::mock::MockRunnerAcknowledgement;

#[cfg(any(test, feature = "test-support"))]
pub use cancel::mock::MockRunnerCancellation;
