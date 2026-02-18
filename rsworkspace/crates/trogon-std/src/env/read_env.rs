use std::env;

/// # Thread Safety
///
/// Does **not** require `Send + Sync`. Add the bounds at your call site:
///
/// ```ignore
/// fn spawn_work<E: ReadEnv + Send + Sync + 'static>(env: Arc<E>) { … }
/// ```
pub trait ReadEnv {
    fn var(&self, key: &str) -> Result<String, env::VarError>;
}
