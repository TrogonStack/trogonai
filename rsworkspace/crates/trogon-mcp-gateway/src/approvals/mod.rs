//! Human-in-the-loop approval flow for policy decisions requiring human consent.
//!
//! After CEL evaluation in `gateway::handle_ingress_inner` (post-`evaluate_hierarchical_policy`),
//! when risk evaluation emits `RequireApproval`, call `ApprovalGate::request_approval`, emit
//! `-32107 approval_required` via `envelope` helpers, and resume or deny based on
//! `ApprovalCoordinator` / `ApprovalNatsListener`.

pub mod audit_sink;
pub mod client;
pub mod coordinator;
pub mod envelope;
pub mod errors;
pub mod gate;
pub mod nats_listener;
pub mod state;
pub mod types;

pub use audit_sink::ApprovalAuditSink;
pub use client::ApprovalClient;
pub use coordinator::ApprovalCoordinator;
pub use envelope::{
    build_approval_required, build_approval_required_step_up, build_approval_required_with_subject,
    jsonrpc_error_rate_limited, jsonrpc_error_with_approval_data,
};
pub use errors::ApprovalError;
pub use gate::{ApprovalGate, CoordinatorApprovalGate};
pub use nats_listener::ApprovalNatsListener;
pub use state::{ApprovalCache, ApprovalStateMachine};
pub use types::{
    ApprovalDecision, ApprovalDecisionKind, ApprovalDecisionMessage, ApprovalRequest, ApprovalSubject,
    ApprovalWaitOutcome, ArgsHash, RequestId, RequestIdError,
};
