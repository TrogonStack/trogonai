use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmBundlePath(PathBuf);

impl WasmBundlePath {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }
}

impl fmt::Display for WasmBundlePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.display().fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_path_identity() {
        let p = WasmBundlePath::new("/tmp/bundles");
        assert_eq!(p.as_path().as_os_str(), "/tmp/bundles");
    }

    #[test]
    fn display_is_path_repr() {
        let p = WasmBundlePath::new("/x/y");
        assert!(format!("{p}").contains("x"));
    }
}
