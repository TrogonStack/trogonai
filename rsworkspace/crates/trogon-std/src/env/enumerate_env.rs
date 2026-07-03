use std::ffi::OsString;

/// Enumeration of the whole environment. Separate from
/// [`ReadEnv`](super::ReadEnv) so a point-lookup double need not implement it.
pub trait EnumerateEnv {
    fn vars(&self) -> Vec<(String, String)>;

    /// Defaults to the UTF-8 pairs from [`vars`](Self::vars); override to
    /// preserve non-Unicode values (as [`SystemEnv`](super::SystemEnv) does).
    fn vars_os(&self) -> Vec<(OsString, OsString)> {
        self.vars()
            .into_iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }
}
