use crate::JsRuntimeError;

/// Why a lease's guest could not be retired.
#[derive(Debug, thiserror::Error)]
pub enum RetireError {
    /// The task that runs the guest ended without confirming it finished.
    #[error("guest task ended without a completion receipt")]
    NoReceipt,
    /// The persistent runtime did not shut down cleanly.
    #[error(transparent)]
    Session(#[from] JsRuntimeError),
}
