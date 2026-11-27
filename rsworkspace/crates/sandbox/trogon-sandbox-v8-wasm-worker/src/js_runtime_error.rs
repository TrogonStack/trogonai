/// Why a JavaScript runtime could not be started or stopped cleanly.
#[derive(Debug, thiserror::Error)]
pub enum JsRuntimeError {
    #[error("a persistent runtime needs a tokio runtime")]
    NoTokioRuntime,
    #[error("failed to start runtime thread: {0}")]
    ThreadSpawn(#[source] std::io::Error),
    #[error("runtime did not start in time")]
    StartTimeout,
    #[error("js execution task panicked: {0}")]
    TaskPanicked(String),
    #[error("persistent runtime thread panicked during shutdown")]
    ShutdownPanicked,
}
