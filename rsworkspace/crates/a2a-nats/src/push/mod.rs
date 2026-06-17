//! Push-notification subsystem value objects + transport-level value types.
//!
//! `caller_id`, dispatch, DLQ, and the live wire shapes that drive the push
//! pipeline land in follow-up PRs alongside their integration harnesses;
//! this slice ships only the validated value-object surface (config id,
//! idempotency key, status transition id, terminal task state, subject,
//! and the header types) so per-operation push PRs can reference them
//! without each one re-deriving the same validation.

pub mod authentication_header;
pub mod idempotency_key_header;
pub mod nats_push_subject;
pub mod push_idempotency_key;
pub mod push_notification_config_id;
pub mod status_transition_id;
pub mod terminal_push_task_state;
