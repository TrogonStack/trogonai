//! What a targeted dispatch gets when the broker sends it again, or sends a new attempt for a released lease.

mod support;

use support::*;
use trogon_sandbox_contract::ProviderCleanupResponse;

#[tokio::test]
async fn a_repeated_targeted_dispatch_returns_the_same_receipt_without_a_second_machine() -> TestResult {
    let env = Env::start().await?;
    let dispatch = dispatch_payload(&env, "repeated", Some("attempt-a"), request_json(), vec![])?;

    let first = env.provider.lease_dispatch(&dispatch).await?;
    let second = env.provider.lease_dispatch(&dispatch).await?;

    assert_eq!(first, second);
    assert_eq!(env.fly.count("POST", "/machines"), 1);
    assert_eq!(env.fly.machines().len(), 1);
    Ok(())
}

/// The ledger gives a lease id one attempt per worker, so an old attempt can never gain authority over a later
/// machine for the same lease.
#[tokio::test]
async fn a_new_attempt_for_a_released_lease_is_refused_without_a_machine() -> TestResult {
    let env = Env::start().await?;
    let first = dispatch_payload(&env, "reused", Some("attempt-a"), request_json(), vec![])?;
    env.provider.lease_dispatch(&first).await?;
    let release = cleanup_request(&env, "reused", "attempt-a", "release")?;
    assert!(matches!(
        env.provider.cleanup_dispatch(&release).await,
        ProviderCleanupResponse::Cleaned { .. }
    ));

    let second = dispatch_payload(&env, "reused", Some("attempt-b"), request_json(), vec![])?;
    let outcome = env.provider.lease_dispatch(&second).await;

    assert!(outcome.is_err(), "{outcome:?}");
    assert_eq!(env.fly.count("POST", "/machines"), 1);
    assert!(env.fly.machines().is_empty());
    Ok(())
}
