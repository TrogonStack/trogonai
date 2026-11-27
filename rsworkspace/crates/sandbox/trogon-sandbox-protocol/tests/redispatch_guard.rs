use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use trogon_sandbox_contract::{LeaseId, ProviderLease, SandboxId};
use trogon_sandbox_protocol::RedispatchGuard;

type Boxed = std::io::Error;
type TestResult = Result<(), Box<dyn std::error::Error>>;

fn grant(sandbox_id: &str) -> Result<ProviderLease, Boxed> {
    Ok(ProviderLease {
        epoch: None,
        sandbox_id: SandboxId::new(sandbox_id).map_err(Boxed::other)?,
        evidence_subject: "e".to_owned(),
        worker_id: None,
        attempt_id: None,
    })
}

#[tokio::test]
async fn a_repeat_gets_the_first_grant_without_launching_again() -> TestResult {
    let guard = RedispatchGuard::new();
    let launches = AtomicU32::new(0);
    let id = LeaseId::new("l");
    let mut ids = Vec::new();
    for _ in 0..3 {
        let lease = guard
            .get_or_launch(&id, || async {
                let n = launches.fetch_add(1, Ordering::SeqCst);
                grant(&format!("sandbox-{n}"))
            })
            .await?;
        ids.push(lease.sandbox_id.to_string());
    }
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert_eq!(ids, ["sandbox-0"; 3]);
    Ok(())
}

#[tokio::test]
async fn a_failed_launch_is_retried_and_a_released_lease_launches_afresh() -> TestResult {
    let guard = RedispatchGuard::new();
    let id = LeaseId::new("l");
    let failed = guard
        .get_or_launch(&id, || async { Err::<ProviderLease, Boxed>(Boxed::other("boom")) })
        .await;
    assert!(failed.is_err());
    let first = guard.get_or_launch(&id, || async { grant("a") }).await?;
    assert_eq!(first.sandbox_id.as_str(), "a");
    guard.forget(&id);
    let second = guard.get_or_launch(&id, || async { grant("b") }).await?;
    assert_eq!(second.sandbox_id.as_str(), "b");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_concurrent_repeat_waits_for_the_first_launch() -> TestResult {
    let guard = Arc::new(RedispatchGuard::new());
    let launches = Arc::new(AtomicU32::new(0));
    let id = LeaseId::new("l");
    let run = |guard: Arc<RedispatchGuard>, launches: Arc<AtomicU32>, id: LeaseId| async move {
        guard
            .get_or_launch(&id, || async {
                launches.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                grant("only")
            })
            .await
    };
    let (a, b) = tokio::join!(
        run(guard.clone(), launches.clone(), id.clone()),
        run(guard.clone(), launches.clone(), id.clone())
    );
    assert_eq!(a?, b?);
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_forgotten_lease_does_not_lose_the_grant_of_its_replacement_when_the_old_launch_fails() -> TestResult {
    let guard = RedispatchGuard::new();
    let id = LeaseId::new("l");
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let mut stale = Box::pin(guard.get_or_launch(&id, || async move {
        let _ = release_rx.await;
        Err::<ProviderLease, Boxed>(Boxed::other("late failure"))
    }));
    assert!(futures::poll!(stale.as_mut()).is_pending());
    guard.forget(&id);
    let replacement = guard.get_or_launch(&id, || async { grant("replacement") }).await?;
    assert_eq!(replacement.sandbox_id.as_str(), "replacement");
    release_tx.send(()).map_err(|_| "the stale launch stopped waiting")?;
    assert!(stale.await.is_err());
    let launches = AtomicU32::new(0);
    let again = guard
        .get_or_launch(&id, || async {
            launches.fetch_add(1, Ordering::SeqCst);
            grant("unexpected")
        })
        .await?;
    assert_eq!(again.sandbox_id.as_str(), "replacement");
    assert_eq!(launches.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_waiting_repeat_runs_its_own_launch_when_the_first_one_fails() -> TestResult {
    let guard = Arc::new(RedispatchGuard::new());
    let id = LeaseId::new("l");
    let first = {
        let guard = guard.clone();
        let id = id.clone();
        tokio::spawn(async move {
            guard
                .get_or_launch(&id, || async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Err::<ProviderLease, Boxed>(Boxed::other("first failed"))
                })
                .await
                .is_err()
        })
    };
    tokio::task::yield_now().await;
    let second = guard.get_or_launch(&id, || async { grant("second") }).await?;
    assert_eq!(second.sandbox_id.as_str(), "second");
    assert!(first.await?);
    Ok(())
}

#[tokio::test]
async fn forgetting_an_unknown_lease_is_a_no_op() -> TestResult {
    let guard = RedispatchGuard::default();
    guard.forget(&LeaseId::new("never-seen"));
    let lease = guard
        .get_or_launch(&LeaseId::new("never-seen"), || async { grant("fresh") })
        .await?;
    assert_eq!(lease.sandbox_id.as_str(), "fresh");
    Ok(())
}

#[tokio::test]
async fn distinct_leases_launch_independently() -> TestResult {
    let guard = RedispatchGuard::new();
    let a = guard
        .get_or_launch(&LeaseId::new("a"), || async { grant("sa") })
        .await?;
    let b = guard
        .get_or_launch(&LeaseId::new("b"), || async { grant("sb") })
        .await?;
    assert_ne!(a.sandbox_id, b.sandbox_id);
    Ok(())
}
