use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The background task that enforces one finite deadline.
#[derive(Debug)]
pub(super) struct Supervisor {
    cancel: CancellationToken,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Supervisor {
    pub(super) fn new(cancel: CancellationToken, task: JoinHandle<()>) -> Self {
        Self {
            cancel,
            task: Mutex::new(Some(task)),
        }
    }

    pub(super) async fn join(&self) {
        self.cancel.cancel();
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
