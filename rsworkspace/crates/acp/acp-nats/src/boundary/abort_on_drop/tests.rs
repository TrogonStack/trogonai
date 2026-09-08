use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn pending_task_with_drop_flag() -> (AbortOnDrop<()>, Arc<AtomicBool>) {
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = SetOnDrop(dropped.clone());
    let guard = AbortOnDrop::new(tokio::spawn(async move {
        let _flag = flag;
        std::future::pending::<()>().await
    }));
    (guard, dropped)
}

#[tokio::test]
async fn drop_aborts_the_task() {
    let (guard, dropped) = pending_task_with_drop_flag();
    drop(guard);
    while !dropped.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn abort_and_wait_stops_a_pending_task() {
    let (guard, dropped) = pending_task_with_drop_flag();
    assert!(!guard.is_finished());
    guard.abort_and_wait().await;
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn finished_task_reports_finished() {
    let handle = tokio::spawn(async {});
    let mut guard = AbortOnDrop::new(handle);
    let _ = guard.handle_mut().await;
    assert!(guard.is_finished());
}

/// Regression: `abort_and_wait` after the handle has already been awaited.
///
/// A caller that races `handle_mut()` in a `select!` and then unconditionally
/// cleans up hits this sequence on the task's normal-exit path. Awaiting a
/// `JoinHandle` twice panics, so `abort_and_wait` must tolerate an
/// already-awaited handle rather than relying on every call site to guard it.
#[tokio::test]
async fn abort_and_wait_tolerates_an_already_awaited_handle() {
    let mut guard = AbortOnDrop::new(tokio::spawn(async {}));
    let _ = guard.handle_mut().await;
    assert!(guard.is_finished());

    guard.abort_and_wait().await;
}
