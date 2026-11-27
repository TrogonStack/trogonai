use crate::{ApiError, LeaseViolationError, OperationState, SandboxState, SpaceId, SpaceIdError};
use trogon_sandbox_protocol::AttemptLedgerError;

/// Why a lease operation failed.
#[derive(Debug, thiserror::Error)]
pub enum SpaceProviderError {
    /// A SPACE call failed.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// The request is not one this worker serves.
    #[error(transparent)]
    Violation(#[from] LeaseViolationError),
    /// The durable dispatch ledger refused or failed.
    #[error(transparent)]
    Ledger(#[from] AttemptLedgerError),
    /// A sandbox identifier is not safe to use.
    #[error(transparent)]
    Id(#[from] SpaceIdError),
    /// The lease is not held by this worker.
    #[error("lease is not held by this worker")]
    NotHeld,
    /// The lease already exists or the worker has no room.
    #[error("lease already active or worker full")]
    Busy,
    /// A cancel arrived while the sandbox was being created.
    #[error("the lease was cancelled while its sandbox was being created")]
    Cancelled,
    /// An operation ended without succeeding.
    #[error("{0}")]
    OperationFailed(String),
    /// An operation did not finish in time.
    #[error("operation {id} did not finish in time (state {state})")]
    OperationTimeout {
        /// The operation.
        id: SpaceId,
        /// The state it was last seen in.
        state: OperationState,
    },
    /// The sandbox vanished while it was awaited.
    #[error("sandbox disappeared")]
    Disappeared,
    /// The sandbox settled in a state other than the one awaited.
    #[error("sandbox is {state} (failure {failure}), expected {target}")]
    UnexpectedState {
        /// The state found.
        state: SandboxState,
        /// The failure code SPACE reported, or `none`.
        failure: String,
        /// The state awaited.
        target: String,
    },
    /// The sandbox did not reach the awaited state in time.
    #[error("sandbox did not reach {target} in time (state {state})")]
    StateTimeout {
        /// The state awaited.
        target: String,
        /// The state it was last seen in.
        state: SandboxState,
    },
    /// The sandbox was still present when the deletion deadline passed.
    #[error("sandbox deletion did not finish in time (state {state})")]
    DeleteTimeout {
        /// The state it was last seen in.
        state: SandboxState,
    },
    /// The sandbox is unknown to SPACE.
    #[error("sandbox is missing")]
    Missing,
    /// The sandbox is in a state the lifecycle change cannot start from.
    #[error("cannot {verb} a sandbox that is {state}")]
    CannotTransition {
        /// The lifecycle verb.
        verb: &'static str,
        /// The sandbox's state.
        state: SandboxState,
    },
    /// A create of unknown outcome could not be reconciled.
    #[error("create outcome is unknown and reconciliation failed: {0}")]
    Reconcile(Box<SpaceProviderError>),
    /// A failed create also failed to clean up after itself.
    #[error("{error}; sandbox cleanup also failed: {cleanup}")]
    CleanupFailed {
        /// Why the create failed.
        error: Box<SpaceProviderError>,
        /// Why the cleanup failed.
        cleanup: Box<SpaceProviderError>,
    },
    /// No single sandbox can be tied to the attempt.
    #[error("cloud create outcome is ambiguous; backend recovery proof is required")]
    AmbiguousCreate,
    /// The lease's sandbox is not the one the attempt created.
    #[error("lease is held by a different backend attempt")]
    ForeignAttempt,
    /// An unfinished create did not stop in time.
    #[error("the unfinished create did not stop in time")]
    AbortTimeout,
}
