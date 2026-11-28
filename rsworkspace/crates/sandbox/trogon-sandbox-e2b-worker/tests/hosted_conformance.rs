//! The conformance suite against a worker leasing from a real E2B account.
//!
//! Skipped unless `SANDBOX_E2B_HOSTED_TEST=1`; see `hosted_support` for the other variables.

mod hosted_support;

use hosted_support::{Harness, TestResult, run_with_exact_cleanup};
use trogon_sandbox_conformance::GrantLedger;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "paid hosted E2B: set SANDBOX_E2B_HOSTED_TEST=1 with E2B_API_KEY and a same-account worker on SANDBOX_CONFORMANCE_NATS_URL, then use --ignored"]
async fn hosted_e2b_conforms_and_every_known_native_grant_is_absent() -> TestResult {
    let harness = Harness::configured().await?;
    let ledger = GrantLedger::default();
    let target = harness.target(ledger.clone())?;
    run_with_exact_cleanup(&harness, &target, &ledger, "conformance").await
}
