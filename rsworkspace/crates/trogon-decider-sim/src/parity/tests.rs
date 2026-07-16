use super::*;
use crate::ir::WireEnvelope;

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
    let native = vec![StepOutcome::Rejected {
        code: "already-exists".to_string(),
        message: "nope".to_string(),
    }];
    let wasm = vec![StepOutcome::Faulted {
        code: "already-exists".to_string(),
        message: "nope".to_string(),
    }];
    let error = compare_outcomes("scenario", native, wasm).unwrap_err();
    assert!(matches!(error, ParityError::Mismatch { index: 0, .. }));
}
