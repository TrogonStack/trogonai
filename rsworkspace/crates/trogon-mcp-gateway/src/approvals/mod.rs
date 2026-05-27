//! Human-in-the-loop approval client, cache, and JSON-RPC `-32107` envelopes.

pub mod client;
pub mod envelope;
pub mod state;
pub mod types;

pub use client::ApprovalClient;
pub use envelope::{
    build_approval_required, build_approval_required_step_up, build_approval_required_with_subject,
    jsonrpc_error_rate_limited, jsonrpc_error_with_approval_data,
};
pub use state::{ApprovalCache, ApprovalStateMachine};
pub use types::{
    ApprovalDecisionKind, ApprovalDecisionMessage, ApprovalError, ApprovalSubject, ApprovalWaitOutcome, ArgsHash,
    RequestId, RequestIdError,
};
