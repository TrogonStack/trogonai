/// A command that cannot be built because it violates the evaluation binding contract.
///
/// The display text is the fixed `EvaluationBindingCommandInvalidError` message; the field name
/// is kept for logs and tests only.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("The command violates the evaluation binding contract.")]
pub struct CommandInvalidError {
    pub field: &'static str,
}

/// Refusals raised by `decide`. Each display text is the message of the matching fault in
/// `proto/trogonai/evaluation/bindings/v1alpha1/faults.proto`. None of them carries the matched
/// term, the rubric, or the offending finding, so a refusal cannot leak the yardstick.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum EvaluationBindingDecideError {
    #[error("The evaluation binding already exists.")]
    AlreadyExists,
    #[error("The evaluation binding does not exist or is unbound.")]
    NotFound,
    #[error("The rubric version named by the binding does not exist or its rubric is retired.")]
    RubricVersionNotFound,
    #[error("The judge must be distinct from every agent it evaluates.")]
    JudgeMatchesSubject,
    #[error("The evaluation result has already been recorded.")]
    ResultAlreadyRecorded,
    #[error("A finding names a surface the evaluated revision does not allow a candidate to edit.")]
    FindingOutsideEditableSurface,
    #[error("A finding matches vocabulary its rubric version withholds.")]
    FindingMatchesWithheldVocabulary,
}

/// A persisted event the fold cannot apply.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum EvaluationBindingEvolveError {
    #[error("evaluation binding event envelope carried no event")]
    MissingEvent,
    #[error("evaluation binding event is missing '{field}'")]
    MissingField { field: &'static str },
    #[error("rubric was bound to a stream that already has a binding")]
    BoundTwice,
    #[error("rubric was unbound without an active binding")]
    UnboundWithoutActiveBinding,
    #[error("evaluation result was recorded without an active binding")]
    RecordedWithoutActiveBinding,
    #[error("evaluation result '{0}' was recorded twice")]
    ResultRecordedTwice(String),
}
