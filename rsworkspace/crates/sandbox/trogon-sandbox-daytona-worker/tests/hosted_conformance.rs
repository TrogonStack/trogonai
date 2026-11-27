//! The conformance suite against a worker leasing from a real Daytona account.
//!
//! Skipped unless `SANDBOX_DAYTONA_HOSTED_TEST=1`; see `hosted_support` for the other variables.

mod hosted_support;
#[path = "hosted_support/prerequisite_diagnosis/mod.rs"]
mod prerequisite_diagnosis;
#[path = "hosted_support/suite_failure_diagnosis/mod.rs"]
mod suite_failure_diagnosis;

use hosted_support::{Harness, IGNORE_REASON, TestResult, run_with_exact_cleanup};
use trogon_sandbox_conformance::GrantLedger;

#[tokio::test]
#[ignore = "runs against a real Daytona account; set SANDBOX_DAYTONA_HOSTED_TEST=1 and use --ignored"]
async fn hosted_daytona_conforms_and_every_matching_native_grant_is_absent() -> TestResult {
    assert!(!IGNORE_REASON.is_empty());
    let harness = Harness::configured().await?;
    let ledger = GrantLedger::default();
    let target = harness.target(ledger.clone())?;
    run_with_exact_cleanup(&harness, &target, &ledger, "conformance").await
}
