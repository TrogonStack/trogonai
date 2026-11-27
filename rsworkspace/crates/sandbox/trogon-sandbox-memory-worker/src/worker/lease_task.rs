use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use super::command_task::{ClosePolicy, CommandTask};
use super::lease::Lease;
use crate::mutex_ext::MutexExt;

/// Runs `future` as a task of `lease`, so closing the lease reaches it. `None` means the lease
/// is closing and the future never started.
pub(crate) async fn run_lease_task<T: Send + 'static>(
    lease: &Arc<Lease>,
    close_policy: ClosePolicy,
    future: impl Future<Output = T> + Send + 'static,
) -> Option<T> {
    let admission = lease.admission.lock().await;
    if lease.closing.load(Ordering::SeqCst) {
        return None;
    }
    let id = uuid::Uuid::now_v7().to_string();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel();
    let (output_tx, output_rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let _ = start_rx.await;
        let _ = output_tx.send(future.await);
    });
    let task = CommandTask::new(
        handle,
        CancellationToken::new(),
        Arc::new(Mutex::new(None)),
        close_policy,
    );
    lease.tasks.lock_clean().insert(id.clone(), task.clone());
    let _ = start_tx.send(());
    drop(admission);
    let output = output_rx.await.ok();
    task.join().await;
    lease.tasks.lock_clean().remove(&id);
    output
}
