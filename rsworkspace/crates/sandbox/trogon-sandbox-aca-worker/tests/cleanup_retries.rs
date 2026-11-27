mod support;

use std::time::Duration;

use support::TestResult;
use support::lease::Lease;
use support::provider::Fixture;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_aca_worker::run_cleanup_retries;
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::Provider;

#[tokio::test(start_paused = true)]
async fn failed_allocations_are_cleaned_up_on_every_tick_until_they_succeed() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.aca.fail_executions();
    fixture.aca.script_delete(Err(500));
    fixture.aca.script_delete(Err(503));
    let outcome = fixture
        .provider
        .lease(&Lease::empty().build()?, &LeaseId::new("lease-stuck"))
        .await;
    assert!(outcome.is_err());
    assert_eq!(fixture.provider.active_lease_count().await, 1);
    let cancel = CancellationToken::new();
    let retries = tokio::spawn(run_cleanup_retries(
        fixture.provider.clone(),
        Duration::from_secs(10),
        cancel.clone(),
    ));

    tokio::time::sleep(Duration::from_secs(25)).await;

    assert_eq!(fixture.provider.active_lease_count().await, 0);
    cancel.cancel();
    retries.await?;
    Ok(())
}
