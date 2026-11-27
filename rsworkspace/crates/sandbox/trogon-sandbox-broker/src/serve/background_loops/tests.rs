use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

#[tokio::test(start_paused = true)]
async fn every_runs_at_once_and_on_each_period_until_cancelled() -> TestResult {
    let tasks = TaskTracker::new();
    let cancel = CancellationToken::new();
    let loops = BackgroundLoops::new(tasks.clone(), cancel.clone());
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&runs);
    loops.every(Duration::from_secs(5), move || {
        let counter = Arc::clone(&counter);
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    tokio::time::sleep(Duration::from_secs(11)).await;
    loops.cancel().cancel();
    tasks.close();
    tasks.wait().await;

    assert_eq!(runs.load(Ordering::SeqCst), 3);
    Ok(())
}

#[tokio::test]
async fn spawn_is_tracked() -> TestResult {
    let tasks = TaskTracker::new();
    let loops = BackgroundLoops::new(tasks.clone(), CancellationToken::new());
    let (sender, receiver) = tokio::sync::oneshot::channel();
    loops.spawn(async move {
        let _sent = sender.send(());
    });
    tasks.close();
    tasks.wait().await;
    receiver.await?;
    Ok(())
}
