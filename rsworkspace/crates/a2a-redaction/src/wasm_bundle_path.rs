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

    // The `SkillId` constructor rejects path separators, `..`, and control
    // characters, so the join operations below cannot escape the configured
    // bundle root — the value is safe to interpolate into the filesystem
    // path by construction.
    pub fn join_skill_wasm(&self, skill: &SkillId) -> PathBuf {
        self.as_path().join(format!("{}.wasm", skill.as_str()))
    }

    pub fn join_skill_manifest(&self, skill: &SkillId) -> PathBuf {
        self.as_path().join(format!("{}.manifest.json", skill.as_str()))
    }

    pub fn join_skill_sig(&self, skill: &SkillId) -> PathBuf {
        self.as_path().join(format!("{}.sig", skill.as_str()))
    }
}

impl fmt::Display for WasmBundlePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.display().fmt(f)
    }
}

#[cfg(test)]
mod tests;
