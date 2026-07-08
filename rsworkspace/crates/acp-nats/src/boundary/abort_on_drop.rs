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

    pub async fn abort_and_wait(mut self) {
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
