use std::sync::Arc;

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use tokio::task::{AbortHandle, JoinHandle};

#[derive(Clone)]
pub(crate) struct ExecutionTask(Arc<ExecutionTaskInner>);

struct ExecutionTaskInner {
    abort: AbortHandle,
    joined: Shared<BoxFuture<'static, ()>>,
}

impl Drop for ExecutionTaskInner {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

impl ExecutionTask {
    pub(crate) fn new(handle: JoinHandle<()>) -> Self {
        Self(Arc::new(ExecutionTaskInner {
            abort: handle.abort_handle(),
            joined: async move {
                let _ = handle.await;
            }
            .boxed()
            .shared(),
        }))
    }

    pub(crate) async fn dispose(&self) {
        self.0.abort.abort();
        self.0.joined.clone().await;
    }
}
