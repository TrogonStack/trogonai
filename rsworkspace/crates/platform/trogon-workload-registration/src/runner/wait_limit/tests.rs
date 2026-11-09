use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

#[tokio::test(start_paused = true)]
async fn expired_authority_refuses_to_poll_the_effect() {
    let called = AtomicBool::new(false);
    let deadline = Instant::now().into_std();
    let result = WaitLimit::effect(Some(deadline), None)
        .run(async {
            called.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await;
    assert!(matches!(result, Err(RunnerError::Expired)));
    assert!(!called.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn both_approval_and_existing_authority_bound_a_pending_effect() {
    for earlier_approval in [true, false] {
        let now = Instant::now();
        let early = (now + Duration::from_secs(2)).into_std();
        let late = (now + Duration::from_secs(4)).into_std();
        let (approval, current) = if earlier_approval { (early, late) } else { (late, early) };
        let result = WaitLimit::effect(Some(approval), Some(current))
            .run(std::future::pending::<Result<(), RunnerError>>())
            .await;
        assert!(matches!(result, Err(RunnerError::Expired)));
        assert_eq!(Instant::now(), Instant::from_std(early));
    }
}

#[tokio::test(start_paused = true)]
async fn an_effect_completing_at_its_authority_deadline_cannot_return_success() {
    let deadline = Instant::now() + Duration::from_secs(2);
    let result = WaitLimit::effect(Some(deadline.into_std()), None)
        .run(async {
            tokio::time::advance(Duration::from_secs(2)).await;
            Ok(())
        })
        .await;
    assert!(matches!(result, Err(RunnerError::Expired)));
}

#[tokio::test(start_paused = true)]
async fn an_unresponsive_effect_is_bounded_even_without_an_authority_lease() {
    let started = Instant::now();
    let result = WaitLimit::effect(None, None)
        .run(std::future::pending::<Result<(), RunnerError>>())
        .await;
    assert!(matches!(result, Err(RunnerError::OperationTimeout)));
    assert_eq!(Instant::now() - started, Duration::from_secs(5));
}
