use std::{
    ffi::OsString,
    path::{Component, Path},
};

use crate::{SessionId, SessionProjectKey, SessionProjectKeyError, SessionSubpath, SessionSubpathError};

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
        let base = resolve_components(projects_dir);
        let target = resolve_components(file_path);
        let common = base
            .iter()
            .zip(&target)
            .take_while(|(left, right)| same_component(left, right))
            .count();
        if common < base.len() {
            return Err(SessionKeyError::OutsideProjects);
        }
        let mut parts = Vec::new();
        for part in &target[common..] {
            parts.push(part.to_str().ok_or(SessionKeyError::NotUnicode)?);
        }
        match parts.as_slice() {
            [project, transcript] => {
                let session = transcript
                    .strip_suffix(".jsonl")
                    .ok_or(SessionKeyError::UnrecognizedPath)?;
                Ok(Self::new(
                    SessionProjectKey::unchecked(*project),
                    SessionId::from(session),
                ))
            }
            [project, session, rest @ ..] if rest.len() >= 2 => {
                let joined = rest.join("/");
                let subpath = joined.strip_suffix(".jsonl").unwrap_or(&joined);
                Ok(
                    Self::new(SessionProjectKey::unchecked(*project), SessionId::from(*session))
                        .with_subpath(SessionSubpath::new(subpath)),
                )
            }
            _ => Err(SessionKeyError::UnrecognizedPath),
        }
    }
}

/// Node `path.resolve` / Python `os.path.abspath` as components: a relative path is joined to the working directory,
/// `.` is dropped and `..` removes the previous component but never a prefix or the root.
fn resolve_components(path: &Path) -> Vec<OsString> {
    let absolute = std::env::current_dir().unwrap_or_default().join(path);
    let mut out: Vec<OsString> = Vec::new();
    let mut floor = 0;
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                out.push(component.as_os_str().to_owned());
                floor = out.len();
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if out.len() > floor {
                    out.pop();
                }
            }
            Component::Normal(name) => out.push(name.to_owned()),
        }
    }
    out
}

fn same_component(left: &OsString, right: &OsString) -> bool {
    if cfg!(windows) {
        left.to_string_lossy().eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}
