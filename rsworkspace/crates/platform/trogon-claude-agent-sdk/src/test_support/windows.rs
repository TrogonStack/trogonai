use crate::{SessionKey, SessionKeyError, SummaryDialect};
use std::path::Path;

/// `SessionKey::from_transcript_path` with the Windows rules, a dialect, a fixed working directory and text paths.
pub fn transcript_key(
    file_path: &str,
    projects_dir: &str,
    cwd: &str,
    dialect: SummaryDialect,
) -> Result<SessionKey, SessionKeyError> {
    SessionKey::from_transcript_path_on(
        Path::new(file_path),
        Path::new(projects_dir),
        Path::new(cwd),
        true,
        dialect,
    )
}

/// The TypeScript resume guard: true when the subpath is skipped as unsafe.
pub fn typescript_subpath_is_unsafe(subpath: &str, session_dir: &str) -> bool {
    crate::session_resume::unsafe_subpath_ts(subpath, Path::new(session_dir), true)
}

/// The Python resume guard: true when the subpath is accepted.
pub fn python_subpath_is_safe(subpath: &str) -> bool {
    crate::session_subpath::python_accepts(subpath)
}

/// `SessionSubpath::checked`, Rust's own rule, for a platform.
pub fn checked_subpath_is_safe(subpath: &str, windows: bool) -> bool {
    crate::session_subpath::is_safe(subpath, windows)
}

/// The name a directory listing compares on: lowercase on Windows, as both SDKs fold it there.
pub fn fold_directory_name(name: &str, windows: bool) -> String {
    crate::session_project_key::fold_directory_name(name, windows)
}
