//! Human-in-the-loop approval workflow for vault credential writes.
//!
//! An [`ApprovalService`] subscribes to NATS JetStream and waits for a human
//! to approve or reject each credential-write proposal before the plaintext is
//! ever written to the vault.
//!
//! # Workflow
//!
//! 1. Agent publishes a [`CreateRequest`] to `vault.proposals.{vault}.create`.
//! 2. [`ApprovalService`] persists the proposal (in-process [`DashMap`]) and
//!    calls [`Notifier::notify_pending`] (e.g. Slack).
//! 3. Human publishes an [`ApproveRequest`] or [`RejectRequest`].
//! 4. On approval: service calls `vault.store(token, plaintext)`, updates
//!    proposal status, and optionally emits an [`AuditPublisher`] event.
//! 5. Agent polls `vault.proposals.{vault}.status.{id}` via request-reply.

pub mod error;
pub mod notifier;
pub mod proposal;
pub mod service;
pub mod stream;
pub mod subjects;

pub use error::ApprovalError;
pub use notifier::{LoggingNotifier, NoopNotifier, Notifier};
#[cfg(feature = "slack")]
pub use notifier::SlackWebhookNotifier;
pub use proposal::{
    ApproveRequest, CreateRequest, Proposal, ProposalId, ProposalStatus, RejectRequest, StatusResponse,
};
pub use service::ApprovalService;
pub use stream::{ensure_proposals_stream, ensure_proposals_stream_with_max_age};
pub use subjects::{PROPOSALS_STREAM, state_update as state_update_subject};
