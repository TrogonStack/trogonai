use std::sync::{Arc, Mutex};

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::UnixSignal;

use crate::mutex_ext::MutexExt;

/// A spawned task of a lease that can be signalled, aborted and joined.
pub(crate) struct CommandTask {
    pub cancel: CancellationToken,
    pub signal: Arc<Mutex<Option<UnixSignal>>>,
    abort: AbortHandle,
    joined: Shared<BoxFuture<'static, ()>>,
    close_policy: ClosePolicy,
}

/// What closing the lease does to a task that is still running.
#[derive(Clone, Copy)]
pub(crate) enum ClosePolicy {
    /// Cancel and abort it.
    Abort,
    /// Cancel it and let it finish its response.
    JoinResponse,
}

impl CommandTask {
    pub(crate) fn new(
        handle: JoinHandle<()>,
        cancel: CancellationToken,
        signal: Arc<Mutex<Option<UnixSignal>>>,
        close_policy: ClosePolicy,
    ) -> Arc<Self> {
        let abort = handle.abort_handle();
        let joined = async move {
            if let Err(error) = handle.await
                && !error.is_cancelled()
            {
                tracing::warn!(%error, "memory command task ended with a panic");
            }
        }
        .boxed()
        .shared();
        Arc::new(Self {
            cancel,
            signal,
            abort,
            joined,
            close_policy,
        })
    }

    /// Kills the task as SIGKILL would.
    pub(crate) fn terminate(&self) {
        *self.signal.lock_clean() = Some(UnixSignal::Kill);
        self.cancel.cancel();
        self.abort.abort();
    }

    /// Asks the task to end because its lease is closing.
    pub(crate) fn request_close(&self) {
        self.cancel.cancel();
        if matches!(self.close_policy, ClosePolicy::Abort) {
            self.terminate();
        }
    }

    /// Waits until the task ended.
    pub(crate) async fn join(&self) {
        self.joined.clone().await;
    }
}
