use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliExecutable(PathBuf);

#[derive(Debug, thiserror::Error)]
pub enum CliExecutableError {
    #[error("the Claude Code executable path is empty")]
    Empty,
    #[error("Windows batch scripts are not supported; use a native Claude Code executable")]
    WindowsBatchScript,
}

impl CliExecutable {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, CliExecutableError> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(CliExecutableError::Empty);
        }
        #[cfg(windows)]
        if path.to_string_lossy().replace('\\', "/").split('/').any(|part| {
            part.split(':').any(|segment| {
                let segment = segment.trim_end_matches(['.', ' ']).to_ascii_lowercase();
                segment.ends_with(".bat") || segment.ends_with(".cmd")
            })
        }) {
            return Err(CliExecutableError::WindowsBatchScript);
        }
        Ok(Self(path))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl Default for CliExecutable {
    fn default() -> Self {
        Self(PathBuf::from(if cfg!(windows) { "claude.exe" } else { "claude" }))
    }
}
