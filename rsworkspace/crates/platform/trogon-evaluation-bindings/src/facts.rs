//! Proven inputs the dispatcher resolves before it dispatches (ADR 0024).
//!
//! A decider consumes its own stream's facts plus opaque inputs. Everything here lives in another
//! stream or in a definition, so the dispatcher reads it, proves it, and hands it over as data.
//! `decide` never reaches for it itself.

use trogonai_proto::agents::agents::v1alpha1::EditableSurface;

/// What the rubrics stream says about the version a binding pins, at dispatch time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RubricVersionStanding {
    /// The version exists on a rubric that is not retired.
    Available,
    /// No such rubric, or no such version of it.
    Unknown,
    /// The rubric exists but is retired, so it accepts no new bindings.
    Retired,
}

/// Whether the judge is one of the agents the binding's subject evaluates.
///
/// A subject that is an agent is compared locally by identity. A subject that is a hierarchy node
/// covers every agent beneath it, and membership lives in the hierarchy, so the dispatcher proves
/// it here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JudgeScope {
    /// The judge is not among the agents the subject evaluates.
    OutsideSubject,
    /// The judge is among the agents the subject evaluates.
    InsideSubject,
}

/// The result of screening a result's findings against the withheld vocabulary of the rubric
/// version the binding pins.
///
/// The decider cannot hold the vocabulary: it is out-of-line bytes behind a claim check, and
/// echoing a term back would defeat the screen. The dispatcher runs the screen over the same
/// findings the command carries and reports only whether any of them matched, never which term or
/// which finding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WithheldVocabularyScreen {
    Clear,
    Matched,
}

/// Facts that let `RecordEvaluationResult` judge its findings.
///
/// `editable_surface` is the surface of the implementation behind the evaluated revision; an
/// implementation that opts out of candidates is the default, empty surface, which no finding can
/// fall inside.
#[derive(Clone, Debug, PartialEq)]
pub struct FindingEvidence {
    pub editable_surface: EditableSurface,
    pub withheld_vocabulary: WithheldVocabularyScreen,
}
