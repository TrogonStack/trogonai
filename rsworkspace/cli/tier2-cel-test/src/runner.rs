//! Fixture-replay engine: evaluate each fixture against a compiled Tier-2
//! CEL bundle using the same [`RealTier2CelEvaluator`] the gateway runs in
//! production, then compare the resulting [`Tier2Decision`] against the
//! fixture's declared expectation.

use a2a_gateway::policy::{RealTier2CelEvaluator, Tier2CelEvaluator, Tier2CompiledBundle, Tier2Decision};

use crate::fixture::{ExpectedOutcome, Fixture, FixtureInputError};

#[derive(Debug, Clone, Copy, Default)]
pub enum OutputFormat {
    #[default]
    Human,
    Tap,
}

/// Outcome of replaying a single fixture.
pub struct FixtureResult {
    pub id: String,
    pub outcome: Result<(), FixtureFailure>,
}

impl FixtureResult {
    pub fn passed(&self) -> bool {
        self.outcome.is_ok()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FixtureFailure {
    #[error("invalid fixture input: {0}")]
    InvalidInput(#[from] FixtureInputError),
    #[error("invalid expected_verdict: {0}")]
    InvalidExpectation(#[from] crate::fixture::ExpectedVerdictError),
    #[error("expected {expected}, got {actual}")]
    Mismatch { expected: String, actual: String },
}

/// Replay every fixture in `fixtures` against `bundle`, returning one
/// [`FixtureResult`] per fixture in input order.
///
/// A fresh [`RealTier2CelEvaluator`] wraps the same compiled bundle for
/// every fixture; the evaluator carries no state across `evaluate` calls
/// (each call re-snapshots the bundle) so this is equivalent to sharing
/// one evaluator instance while keeping the call-site simple.
pub fn run_suite(bundle: Tier2CompiledBundle, fixtures: &[Fixture]) -> Vec<FixtureResult> {
    let evaluator = RealTier2CelEvaluator::new(bundle);
    fixtures
        .iter()
        .map(|fixture| FixtureResult {
            id: fixture.id.clone(),
            outcome: run_fixture(&evaluator, fixture),
        })
        .collect()
}

fn run_fixture(evaluator: &RealTier2CelEvaluator, fixture: &Fixture) -> Result<(), FixtureFailure> {
    let ctx = fixture.input.to_evaluation_context()?;
    let expected = fixture.expected_verdict.to_outcome()?;
    let actual = evaluator.evaluate(&ctx);
    match (&expected, &actual) {
        (ExpectedOutcome::Allow, Tier2Decision::Allow) => Ok(()),
        (ExpectedOutcome::Deny { rule: expected_rule }, Tier2Decision::Deny { rule: actual_rule })
            if expected_rule == actual_rule =>
        {
            Ok(())
        }
        _ => Err(FixtureFailure::Mismatch {
            expected: describe_expected(&expected),
            actual: describe_actual(&actual),
        }),
    }
}

fn describe_expected(expected: &ExpectedOutcome) -> String {
    match expected {
        ExpectedOutcome::Allow => "Allow".to_string(),
        ExpectedOutcome::Deny { rule } => format!("Deny{{rule: {rule}}}"),
    }
}

fn describe_actual(actual: &Tier2Decision) -> String {
    match actual {
        Tier2Decision::Allow => "Allow".to_string(),
        Tier2Decision::Deny { rule } => format!("Deny{{rule: {rule}}}"),
    }
}

/// Render results to stdout/stderr in the requested format, mirroring
/// `trogon-decider-test`'s human (`PASS`/`FAIL` on stdout/stderr) and TAP
/// (`ok`/`not ok`) dual output. Returns the number of failed fixtures.
pub fn report(suite_name: &str, results: &[FixtureResult], format: OutputFormat) -> usize {
    if matches!(format, OutputFormat::Tap) {
        println!("# {suite_name}");
        println!("1..{}", results.len());
    }
    for (index, result) in results.iter().enumerate() {
        match (&result.outcome, format) {
            (Ok(()), OutputFormat::Tap) => println!("ok {} - {}", index + 1, result.id),
            (Ok(()), OutputFormat::Human) => println!("PASS {}", result.id),
            (Err(error), OutputFormat::Tap) => println!("not ok {} - {}: {error}", index + 1, result.id),
            (Err(error), OutputFormat::Human) => eprintln!("FAIL {}: {error}", result.id),
        }
    }
    results.iter().filter(|result| !result.passed()).count()
}

#[cfg(test)]
mod tests;
