use std::{
    ffi::OsString,
    path::{Component, Path},
};

use crate::{
    SessionId, SessionProjectKey, SessionProjectKeyError, SessionSubpath, SessionSubpathError, SummaryDialect,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SessionKey {
    project_key: SessionProjectKey,
    session_id: SessionId,
    subpath: Option<SessionSubpath>,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionKeyError {
    #[error("session id is not a safe file component")]
    UnsafeSessionId,
    #[error("transcript path is not inside the configured projects directory")]
    OutsideProjects,
    #[error("unrecognized transcript path")]
    UnrecognizedPath,
    #[error("transcript path is not Unicode")]
    NotUnicode,
    #[error(transparent)]
    Project(#[from] SessionProjectKeyError),
    #[error(transparent)]
    Subpath(#[from] SessionSubpathError),
}

impl SessionKey {
    /// A key for a session, as upstream builds it: the session id is not validated.
    ///
    /// Stores that turn a key into a file path must apply their own safety rules; [`SessionKey::checked`] is the
    /// opt-in constructor that rejects an id that is not a safe file name component.
    pub fn new(project_key: SessionProjectKey, session_id: SessionId) -> Self {
        Self {
            project_key,
            session_id,
            subpath: None,
        }
    }

    /// A key whose session id must be a safe file name component: not empty, not `.` or `..`, no separator, control
    /// or Windows-reserved character, no trailing dot or space and not a reserved device name.
    pub fn checked(project_key: SessionProjectKey, session_id: SessionId) -> Result<Self, SessionKeyError> {
        if !crate::session_path_component::is_safe(session_id.as_str()) {
            return Err(SessionKeyError::UnsafeSessionId);
        }
        Ok(Self::new(project_key, session_id))
    }

    pub fn with_subpath(mut self, subpath: SessionSubpath) -> Self {
        self.subpath = Some(subpath);
        self
    }
    pub fn main(&self) -> Self {
        Self {
            project_key: self.project_key.clone(),
            session_id: self.session_id.clone(),
            subpath: None,
        }
    }
    pub fn project_key(&self) -> &SessionProjectKey {
        &self.project_key
    }
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    pub fn subpath(&self) -> Option<&SessionSubpath> {
        self.subpath.as_ref()
    }

    /// Upstream `filePathToSessionKey`: both paths are resolved against the working directory when relative and
    /// normalized lexically (`.`, `..` and repeated separators), then the transcript path is read relative to the
    /// projects directory. `project/<id>.jsonl` is a main transcript and `project/<id>/<two or more parts>` a subagent
    /// transcript. Nothing in the path is validated beyond that shape.
    pub fn from_transcript_path(file_path: &Path, projects_dir: &Path) -> Result<Self, SessionKeyError> {
        Self::from_transcript_path_with(file_path, projects_dir, SummaryDialect::TypeScript)
    }

    /// [`SessionKey::from_transcript_path`] for a dialect. The SDKs derive the same key everywhere except one Windows
    /// case: Python reads a part with a colon second, such as `a:x`, as a drive, so its relative path restarts or
    /// splits there. The Python dialect follows CPython 3.12, the version the crate is pinned to.
    pub fn from_transcript_path_with(
        file_path: &Path,
        projects_dir: &Path,
        dialect: SummaryDialect,
    ) -> Result<Self, SessionKeyError> {
        let cwd = std::env::current_dir().unwrap_or_default();
        Self::from_transcript_path_on(file_path, projects_dir, &cwd, cfg!(windows), dialect)
    }

    /// [`SessionKey::from_transcript_path`] for an explicit working directory and platform, so the Windows rules
    /// (case-insensitive comparison, both separators, drives and UNC shares) run on any host.
    pub(crate) fn from_transcript_path_on(
        file_path: &Path,
        projects_dir: &Path,
        cwd: &Path,
        windows: bool,
        dialect: SummaryDialect,
    ) -> Result<Self, SessionKeyError> {
        let parts = if windows {
            let (Some(file), Some(base), Some(cwd)) = (file_path.to_str(), projects_dir.to_str(), cwd.to_str()) else {
                return Err(SessionKeyError::NotUnicode);
            };
            let parts =
                crate::windows_path::components_below(base, file, cwd).ok_or(SessionKeyError::OutsideProjects)?;
            match dialect {
                SummaryDialect::TypeScript if crate::windows_path::is_absolute(&parts.join("\\")) => {
                    return Err(SessionKeyError::UnrecognizedPath);
                }
                SummaryDialect::TypeScript => parts,
                SummaryDialect::Python => crate::windows_path::python_relative_parts(&parts),
            }
        } else {
            let base = resolve_components(projects_dir, cwd);
            let target = resolve_components(file_path, cwd);
            if !target.starts_with(&base) {
                return Err(SessionKeyError::OutsideProjects);
            }
            let mut parts = Vec::new();
            for part in &target[base.len()..] {
                parts.push(part.to_str().ok_or(SessionKeyError::NotUnicode)?.to_string());
            }
            parts
        };
        let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
        match parts.as_slice() {
            [project, transcript] => {
                let session = transcript
                    .strip_suffix(".jsonl")
                    .ok_or(SessionKeyError::UnrecognizedPath)?;
                Ok(Self::new(SessionProjectKey::new(*project), SessionId::from(session)))
            }
            [project, session, rest @ ..] if rest.len() >= 2 => {
                let joined = rest.join("/");
                let subpath = joined.strip_suffix(".jsonl").unwrap_or(&joined);
                Ok(Self::new(SessionProjectKey::new(*project), SessionId::from(*session))
                    .with_subpath(SessionSubpath::new(subpath)))
            }
            _ => Err(SessionKeyError::UnrecognizedPath),
        }
    }
}

/// Node `path.resolve` / Python `os.path.abspath` as components: a relative path is joined to the working directory,
/// `.` is dropped and `..` removes the previous component but never the root.
fn resolve_components(path: &Path, cwd: &Path) -> Vec<OsString> {
    let absolute = cwd.join(path);
    let mut out: Vec<OsString> = Vec::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => out.push(component.as_os_str().to_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                if out.len() > 1 {
                    out.pop();
                }
            }
            Component::Normal(name) => out.push(name.to_owned()),
        }
    }
    out
}
