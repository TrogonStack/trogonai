use thiserror::Error;
use trogonai_session_contracts::SessionId;

use crate::state::{CancelState, ForceSwitchState, SwitchState};

#[derive(Debug, Error)]
pub enum SwitchingError {
    #[error("session {session_id} is busy processing another mutating operation")]
    SessionBusy { session_id: SessionId },

    #[error("switch blocked until safe: {detail}")]
    BlockedUntilSafe { detail: String },

    #[error("switch requires user confirmation: {detail}")]
    ConfirmationRequired { detail: String },

    #[error("force switch rejected: {detail}")]
    ForceSwitchRejected { detail: String },

    #[error("invalid switch state transition from {from:?} to {to:?}")]
    InvalidSwitchTransition { from: SwitchState, to: SwitchState },

    #[error("invalid cancel state transition from {from:?} to {to:?}")]
    InvalidCancelTransition { from: CancelState, to: CancelState },

    #[error("invalid force switch state transition from {from:?} to {to:?}")]
    InvalidForceSwitchTransition {
        from: ForceSwitchState,
        to: ForceSwitchState,
    },

    #[error("target model not found: {model_id}")]
    TargetModelNotFound { model_id: String },

    #[error("continuity checkpoint failed: {detail}")]
    CheckpointFailed { detail: String },

    /// A stage AFTER `runner_attached` failed and Trogonai restored the previous runner
    /// binding (§ "rolled_back: Trogonai intento cambiar, fallo una etapa posterior y
    /// restauro el binding anterior"). The canonical session stays consistent on the
    /// previous binding; the switch can be retried.
    #[error("switch rolled back to previous binding: {detail}")]
    SwitchRolledBack { detail: String },

    #[error("runner acknowledgement failed: {detail}")]
    RunnerAcknowledgementFailed { detail: String },

    #[error("runner attach failed for {runner_id}: {detail}")]
    RunnerAttachFailed { runner_id: String, detail: String },

    #[error("runner detach failed for {runner_id}: {detail}")]
    RunnerDetachFailed { runner_id: String, detail: String },

    #[error("session kernel error: {0}")]
    Kernel(#[from] trogonai_session_kernel::SessionKernelError),

    #[error("capability error: {0}")]
    Capability(#[from] trogonai_capabilities::CapabilityError),

    #[error("projection error: {0}")]
    Projection(#[from] trogonai_session_projection::ProjectionError),

    #[error("artifact store error: {0}")]
    Artifact(#[from] trogonai_artifacts::ArtifactStoreError),

    #[error("missing session field: {0}")]
    MissingField(&'static str),

    #[error("protobuf decode failed: {0}")]
    Decode(String),

    #[error("protobuf encode failed: {0}")]
    Encode(String),

    #[error("runner binding store failed: {0}")]
    RunnerBindingStore(String),

    #[error("runner binding load failed: {0}")]
    RunnerBindingLoad(String),

    #[error("cancel operation failed: {detail}")]
    CancelFailed { detail: String },

    #[error("operation requires reconciliation: {detail}")]
    RequiresReconciliation { detail: String },
}
