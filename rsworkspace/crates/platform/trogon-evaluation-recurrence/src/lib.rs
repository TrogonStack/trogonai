//! Finding recurrence projection for `GetFindingRecurrence` (ADR 0072).
//!
//! Recurrence is a projection over `EvaluationResultRecorded` events, never stored state, so it is
//! recomputed from the evaluation bindings stream (ADR 0024). A result does not name its rubric
//! version, only its binding does, so the fold reads `RubricBound` alongside the results.
//!
//! Findings exist only in the evaluation streams and must never reach an agent, proposal, session,
//! or improver stream or query (ADR 0024, ADR 0025). This crate therefore depends on the
//! evaluation protos alone and exposes no agent-side type.
//!
//! [`RecurrenceApply`] plugs the fold into `trogon-decider-nats`'s `Projector`, and
//! [`RebuildFromZero`] is the checkpoint store that makes every catch-up replay from the start.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod apply;
mod projection;

pub use apply::{RebuildFromZero, RecurrenceApply, RecurrenceApplyError};
pub use projection::{FindingRecurrence, FindingRecurrenceProjection, RecurrencePage, RecurrenceTarget, RubricKey};
