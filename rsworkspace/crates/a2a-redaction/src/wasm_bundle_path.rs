use std::fmt;
use std::path::{Path, PathBuf};

use crate::skill_id::SkillId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmBundlePath(PathBuf);

impl WasmBundlePath {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }

    pub fn join_skill_wasm(&self, skill: &SkillId) -> PathBuf {
        self.as_path().join(format!("{}.wasm", skill.as_str()))
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

    #[test]
    fn skill_wasm_path_appends_slug() {
        let p = WasmBundlePath::new("/b");
        let got = p.join_skill_wasm(&SkillId::new("risk"));
        assert_eq!(got.as_os_str(), "/b/risk.wasm");
    }
}
