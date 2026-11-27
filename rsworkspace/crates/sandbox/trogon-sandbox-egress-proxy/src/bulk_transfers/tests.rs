use super::*;

#[tokio::test]
async fn a_step_that_finishes_returns_its_value() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(bounded("the step", async { 7 }).await?, 7);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_step_that_never_finishes_is_reported() {
    let outcome = bounded("the step", std::future::pending::<()>()).await;
    assert!(matches!(outcome, Err(BoundExceededError(_))));
}

#[tokio::test(start_paused = true)]
async fn a_timed_case_waiting_behind_a_bulk_transfer_is_reported() -> Result<(), Box<dyn std::error::Error>> {
    let lock = RwLock::new(());
    let held = bulk_on(&lock).await?;
    let outcome = timed_on(&lock).await;
    assert!(matches!(outcome, Err(BoundExceededError(_))));
    drop(held);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_bulk_transfer_waiting_behind_a_timed_case_is_reported() -> Result<(), Box<dyn std::error::Error>> {
    let lock = RwLock::new(());
    let held = timed_on(&lock).await?;
    let outcome = bulk_on(&lock).await;
    assert!(matches!(outcome, Err(BoundExceededError(_))));
    drop(held);
    Ok(())
}
