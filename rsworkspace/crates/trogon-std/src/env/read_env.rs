use std::env;
use std::ffi::OsString;

/// # Thread Safety
///
/// Does **not** require `Send + Sync`. Add the bounds at your call site:
///
/// ```text
/// fn spawn_work<E: ReadEnv + Send + Sync + 'static>(env: Arc<E>) { … }
/// ```
pub trait ReadEnv {
    fn var(&self, key: &str) -> Result<String, env::VarError>;

    /// Defaults to the UTF-8 value from [`var`](Self::var); override to preserve
    /// non-Unicode values (as [`SystemEnv`](super::SystemEnv) does).
    fn var_os(&self, key: &str) -> Option<OsString> {
        self.var(key).ok().map(OsString::from)
    }

    /// Defaults to empty — a point-lookup double models no enumerable set.
    /// Override to expose every variable (as [`SystemEnv`](super::SystemEnv)
    /// and [`InMemoryEnv`](super::InMemoryEnv) do).
    fn vars(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Defaults to the UTF-8 pairs from [`vars`](Self::vars); override to
    /// preserve non-Unicode values (as [`SystemEnv`](super::SystemEnv) does).
    fn vars_os(&self) -> Vec<(OsString, OsString)> {
        self.vars()
            .into_iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }
}
