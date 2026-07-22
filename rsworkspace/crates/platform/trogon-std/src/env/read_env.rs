use std::env;
use std::ffi::OsString;

/// # Thread Safety
///
/// Does **not** require `Send + Sync`. Add the bounds at your call site:
///
/// ```text
/// fn spawn_work<E: ReadEnv + Send + Sync + 'static>(env: Arc<E>) { … }
/// ```
/// Point lookup of a single variable. Enumeration of the whole environment is a
/// separate capability, [`EnumerateEnv`](super::EnumerateEnv), so a double that
/// only answers lookups need not model a key set it does not have.
pub trait ReadEnv {
    fn var(&self, key: &str) -> Result<String, env::VarError>;

    /// Defaults to the UTF-8 value from [`var`](Self::var); override to preserve
    /// non-Unicode values (as [`SystemEnv`](super::SystemEnv) does).
    fn var_os(&self, key: &str) -> Option<OsString> {
        self.var(key).ok().map(OsString::from)
    }
}
