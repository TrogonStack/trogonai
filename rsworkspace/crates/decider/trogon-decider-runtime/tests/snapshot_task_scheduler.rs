//! Scheduling without an async runtime cancels snapshot writes.

use tokio::sync::oneshot;
use trogon_decider_runtime::{DrainableSnapshotTaskScheduler, SnapshotTaskScheduler};

#[test]
fn scheduling_without_runtime_cancels_the_snapshot_task() {
    let scheduler = DrainableSnapshotTaskScheduler::new();
    let (completed, mut completion) = oneshot::channel();

    scheduler.schedule(async move {
        let _ = completed.send(());
    });

    assert_eq!(completion.try_recv(), Err(oneshot::error::TryRecvError::Closed));
}
