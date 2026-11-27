use std::path::PathBuf;

/// Files of PEM certificates appended to the trust bundle the workload is pointed at, for hosts the proxy tunnels
/// without terminating, such as one behind an internal CA.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ExtraCaCertPaths(Vec<PathBuf>);

impl ExtraCaCertPaths {
    pub fn new(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self(paths.into_iter().collect())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_slice(&self) -> &[PathBuf] {
        &self.0
    }
}
