use tokio::task::JoinHandle;

/// Aborts the wrapped task when dropped.
///
/// `connect_agent_boundary` races `main_fn` against transport EOF; when EOF
/// wins, `main_fn` is dropped mid-await and any cleanup code after its select
/// never runs. Tasks spawned inside `main_fn` must therefore be tied to its
/// lifetime, or they outlive the connection they serve.
pub struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> AbortOnDrop<T> {
    pub fn new(handle: JoinHandle<T>) -> Self {
        Self(handle)
    }

    pub fn handle_mut(&mut self) -> &mut JoinHandle<T> {
        &mut self.0
    }

    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }

    /// Aborts the task and waits for it to stop.
    ///
    /// A finished task is left alone: awaiting a `JoinHandle` a second time
    /// panics, and a caller that raced [`Self::handle_mut`] in a `select!` has
    /// already awaited it on the normal-exit path. Guarding here rather than at
    /// every call site keeps that panic from being one forgotten `is_finished`
    /// check away. Nothing is lost by skipping: the join result is discarded.
    pub async fn abort_and_wait(mut self) {
        if self.0.is_finished() {
            return;
        }
        self.0.abort();
        let _ = (&mut self.0).await;
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests;
