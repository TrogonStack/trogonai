//! Runs a [`ScenarioIr`] scenario through both a native decider bundle and a compiled wasm
//! component, asserting the two produce identical outcomes.
//!
//! This is the piece that turns "the same behavior is specified twice" into a test that fails
//! on divergence: without it, a codec bug, a codegen bug, or a WIT regression that only breaks
//! one of the two paths surfaces as silent drift rather than a failing assertion.

use crate::host::SimInstance;
use crate::ir::{ScenarioIr, StepOutcome, StreamIdOutcome};
use crate::native::{NativeDeciderBundle, NativeRunError, run_native};
use crate::scenario::ScenarioError;

/// A scenario's native and wasm runs disagreed, or one of them failed to run at all.
#[derive(Debug, thiserror::Error)]
pub enum ParityError {
    /// The native runner failed before it could produce any step outcomes.
    #[error("{scenario}: native run failed: {source}")]
    Native {
        /// The scenario's name.
        scenario: String,
        /// The native runner's underlying failure.
        #[source]
        source: NativeRunError,
    },
    /// The wasm runner failed before it could produce any step outcomes.
    #[error("{scenario}: wasm run failed: {source}")]
    Wasm {
        /// The scenario's name.
        scenario: String,
        /// The wasm runner's underlying failure.
        #[source]
        source: Box<ScenarioError>,
    },
    /// The native and wasm runners produced diverging stream-id outcomes for the scenario's
    /// first command, including one runner producing an outcome the other did not.
    #[error("{scenario}: stream id mismatch: native={native:?}, wasm={wasm:?}")]
    StreamIdMismatch {
        /// The scenario's name.
        scenario: String,
        /// The native runner's stream-id outcome, if it produced one.
        native: Option<Box<StreamIdOutcome>>,
        /// The wasm runner's stream-id outcome, if it produced one.
        wasm: Option<Box<StreamIdOutcome>>,
    },
    /// Both runners agreed on a stream-id outcome, but it was not a resolution matching the
    /// scenario's declared [`ScenarioIr::stream_id`].
    #[error("{scenario}: declared stream id '{declared}' does not match outcome {outcome:?}")]
    StreamIdDeclaredMismatch {
        /// The scenario's name.
        scenario: String,
        /// The scenario's declared stream id.
        declared: String,
        /// The outcome both runners agreed on instead of resolving the declared id.
        outcome: Box<StreamIdOutcome>,
    },
    /// The native and wasm runners produced a different number of step outcomes.
    #[error("{scenario}: native produced {native_len} step outcome(s), wasm produced {wasm_len}")]
    StepCountMismatch {
        /// The scenario's name.
        scenario: String,
        /// The number of step outcomes the native runner produced.
        native_len: usize,
        /// The number of step outcomes the wasm runner produced.
        wasm_len: usize,
    },
    /// The native and wasm runners produced different outcomes for the same step.
    #[error("{scenario}: step {index} outcome mismatch: native={native:?}, wasm={wasm:?}")]
    Mismatch {
        /// The scenario's name.
        scenario: String,
        /// The zero-based index of the mismatched step.
        index: usize,
        /// The native runner's outcome for this step.
        native: Box<StepOutcome>,
        /// The wasm runner's outcome for this step.
        wasm: Box<StepOutcome>,
    },
}

/// Runs `scenario` through both a native [`NativeDeciderBundle`] and a wasm component instance,
/// returning an error on the first divergence between the two runners' outcomes.
///
/// A codec, codegen, or WIT-level divergence between the native decider and the compiled wasm
/// component makes this return `Err`, rather than each runner separately reporting success
/// against its own declared expectations.
pub fn assert_parity<N: NativeDeciderBundle, T>(
    scenario: &ScenarioIr,
    instance: &mut SimInstance<T>,
) -> Result<(), ParityError> {
    let native = run_native::<N>(scenario).map_err(|source| ParityError::Native {
        scenario: scenario.name.clone(),
        source,
    })?;
    let wasm = scenario.run_wasm(instance).map_err(|source| ParityError::Wasm {
        scenario: scenario.name.clone(),
        source: Box::new(source),
    })?;

    compare_stream_ids(
        &scenario.name,
        scenario.stream_id.as_deref(),
        native.stream_id.as_ref(),
        wasm.stream_id.as_ref(),
    )?;
    compare_outcomes(&scenario.name, native.steps, wasm.steps)
}

/// Compares a native and a wasm run's stream-id outcome, independent of how each was produced.
///
/// One runner producing an outcome the other did not is a divergence, the same as two different
/// outcomes. Neither runner producing one (a scenario with no commands) is the only silent case:
/// with nothing resolved, a declared [`ScenarioIr::stream_id`] has nothing to be checked against.
///
/// Kept separate from [`assert_parity`] so the comparison itself can be exercised without a real
/// native decider or wasm component instance, the same way [`compare_outcomes`] is.
fn compare_stream_ids(
    scenario_name: &str,
    declared: Option<&str>,
    native: Option<&StreamIdOutcome>,
    wasm: Option<&StreamIdOutcome>,
) -> Result<(), ParityError> {
    let outcome = match (native, wasm) {
        (None, None) => None,
        (Some(native), Some(wasm)) if native == wasm => Some(native),
        (native, wasm) => {
            return Err(ParityError::StreamIdMismatch {
                scenario: scenario_name.to_string(),
                native: native.cloned().map(Box::new),
                wasm: wasm.cloned().map(Box::new),
            });
        }
    };

    if let (Some(declared), Some(outcome)) = (declared, outcome) {
        match outcome {
            StreamIdOutcome::Resolved(resolved) if resolved == declared => {}
            outcome => {
                return Err(ParityError::StreamIdDeclaredMismatch {
                    scenario: scenario_name.to_string(),
                    declared: declared.to_string(),
                    outcome: Box::new(outcome.clone()),
                });
            }
        }
    }

    Ok(())
}

/// Compares a native and a wasm run's step outcomes, independent of how each was produced.
///
/// Kept separate from [`assert_parity`] so the comparison itself (step count, then per-step
/// equality) can be exercised without a real native decider or wasm component instance.
fn compare_outcomes(scenario_name: &str, native: Vec<StepOutcome>, wasm: Vec<StepOutcome>) -> Result<(), ParityError> {
    if native.len() != wasm.len() {
        return Err(ParityError::StepCountMismatch {
            scenario: scenario_name.to_string(),
            native_len: native.len(),
            wasm_len: wasm.len(),
        });
    }

    for (index, (native_outcome, wasm_outcome)) in native.iter().zip(wasm.iter()).enumerate() {
        if native_outcome != wasm_outcome {
            return Err(ParityError::Mismatch {
                scenario: scenario_name.to_string(),
                index,
                native: Box::new(native_outcome.clone()),
                wasm: Box::new(wasm_outcome.clone()),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
