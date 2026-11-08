//! Deciders for the evaluation bindings stream (ADR 0072).
//!
//! One stream per `EvaluationBindingId` holds `RubricBound`, `RubricUnbound`, and
//! `EvaluationResultRecorded`. Three commands drive it: [`BindRubric`], [`UnbindRubric`], and
//! [`RecordEvaluationResult`].
//!
//! The rubric stream, the evaluated revision's editable surface, and a rubric version's withheld
//! vocabulary all live outside this stream. Per ADR 0024 they reach a decider as proven inputs
//! (see [`facts`]) that the dispatcher resolves before dispatch; nothing here reads another stream.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod bind_rubric;
mod error;
pub mod facts;
mod record_evaluation_result;
mod state;
mod surface;
mod unbind_rubric;

#[cfg(test)]
mod fixtures;

pub use bind_rubric::BindRubric;
pub use error::{CommandInvalidError, EvaluationBindingDecideError, EvaluationBindingEvolveError};
pub use facts::{FindingEvidence, JudgeScope, RubricVersionStanding, WithheldVocabularyScreen};
pub use record_evaluation_result::RecordEvaluationResult;
pub use state::{BindingStatus, EvaluationBindingState, evolve, initial_state};
pub use unbind_rubric::UnbindRubric;
