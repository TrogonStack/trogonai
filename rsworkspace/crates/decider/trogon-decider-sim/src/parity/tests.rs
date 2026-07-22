use super::*;
use crate::ir::{DomainErrorOutcome, WireEnvelope};

fn events_outcome(type_url: &str) -> StepOutcome {
    StepOutcome::Events(vec![WireEnvelope::new(type_url, Vec::new())])
}

#[test]
fn compare_outcomes_passes_when_identical() {
    let native = vec![events_outcome("a")];
    let wasm = vec![events_outcome("a")];
    compare_outcomes("scenario", native, wasm).expect("identical outcomes match");
}

#[test]
fn compare_outcomes_reports_step_count_mismatch() {
    let native = vec![events_outcome("a")];
    let wasm = vec![events_outcome("a"), events_outcome("b")];
    let error = compare_outcomes("scenario", native, wasm).unwrap_err();
    assert!(matches!(
        error,
        ParityError::StepCountMismatch {
            native_len: 1,
            wasm_len: 2,
            ..
        }
    ));
}

#[test]
fn compare_outcomes_reports_the_first_mismatched_step() {
    let native = vec![events_outcome("a"), events_outcome("b")];
    let wasm = vec![events_outcome("a"), events_outcome("different")];
    let error = compare_outcomes("scenario", native, wasm).unwrap_err();
    assert!(matches!(error, ParityError::Mismatch { index: 1, .. }));
}

#[test]
fn compare_outcomes_distinguishes_rejected_from_faulted() {
    let native = vec![StepOutcome::Rejected(DomainErrorOutcome {
        code: "already-exists".to_string(),
        message: "nope".to_string(),
        details: Vec::new(),
    })];
    let wasm = vec![StepOutcome::Faulted(DomainErrorOutcome {
        code: "already-exists".to_string(),
        message: "nope".to_string(),
        details: Vec::new(),
    })];
    let error = compare_outcomes("scenario", native, wasm).unwrap_err();
    assert!(matches!(error, ParityError::Mismatch { index: 0, .. }));
}

#[test]
fn compare_outcomes_distinguishes_by_details() {
    let native = vec![StepOutcome::Rejected(DomainErrorOutcome {
        code: "already-exists".to_string(),
        message: "nope".to_string(),
        details: vec![("cause".to_string(), "duplicate id".to_string())],
    })];
    let wasm = vec![StepOutcome::Rejected(DomainErrorOutcome {
        code: "already-exists".to_string(),
        message: "nope".to_string(),
        details: Vec::new(),
    })];
    let error = compare_outcomes("scenario", native, wasm).unwrap_err();
    assert!(matches!(error, ParityError::Mismatch { index: 0, .. }));
}

#[test]
fn compare_stream_ids_passes_when_neither_runner_resolved_one() {
    compare_stream_ids("scenario", None, None, None).expect("no stream id to compare");
}

#[test]
fn compare_stream_ids_passes_when_identical_and_undeclared() {
    let outcome = StreamIdOutcome::Resolved("backup".to_string());
    compare_stream_ids("scenario", None, Some(&outcome), Some(&outcome)).expect("identical ids match");
}

#[test]
fn compare_stream_ids_reports_native_wasm_divergence() {
    let native = StreamIdOutcome::Resolved("backup".to_string());
    let wasm = StreamIdOutcome::Resolved("other".to_string());
    let error = compare_stream_ids("scenario", None, Some(&native), Some(&wasm)).unwrap_err();
    assert!(matches!(error, ParityError::StreamIdMismatch { .. }));
}

#[test]
fn compare_stream_ids_reports_a_declared_mismatch_even_when_runners_agree() {
    let resolved = StreamIdOutcome::Resolved("backup".to_string());
    let error = compare_stream_ids("scenario", Some("other"), Some(&resolved), Some(&resolved)).unwrap_err();
    assert!(matches!(error, ParityError::StreamIdDeclaredMismatch { .. }));
}

#[test]
fn compare_stream_ids_passes_when_declared_matches_both_runners() {
    let resolved = StreamIdOutcome::Resolved("backup".to_string());
    compare_stream_ids("scenario", Some("backup"), Some(&resolved), Some(&resolved)).expect("declared id matches");
}

#[test]
fn compare_stream_ids_reports_a_one_sided_outcome_as_divergence() {
    let resolved = StreamIdOutcome::Resolved("backup".to_string());

    let error = compare_stream_ids("scenario", None, Some(&resolved), None).unwrap_err();
    assert!(
        matches!(&error, ParityError::StreamIdMismatch { wasm: None, .. }),
        "{error}"
    );

    let error = compare_stream_ids("scenario", None, None, Some(&resolved)).unwrap_err();
    assert!(
        matches!(&error, ParityError::StreamIdMismatch { native: None, .. }),
        "{error}"
    );
}

#[test]
fn compare_stream_ids_reports_a_declared_mismatch_when_both_runners_failed() {
    let failed = StreamIdOutcome::Failed(DomainErrorOutcome {
        code: "boom".to_string(),
        message: "stream id resolution failed".to_string(),
        details: Vec::new(),
    });
    let error = compare_stream_ids("scenario", Some("backup"), Some(&failed), Some(&failed)).unwrap_err();
    assert!(
        matches!(&error, ParityError::StreamIdDeclaredMismatch { .. }),
        "{error}"
    );
}
