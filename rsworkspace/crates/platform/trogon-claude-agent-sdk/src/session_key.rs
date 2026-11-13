use std::path::{Component, Path};

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
    pub fn new(project_key: SessionProjectKey, session_id: SessionId) -> Result<Self, SessionKeyError> {
        let value = session_id.as_str();
        if !crate::session_path_component::is_safe(value) {
            return Err(SessionKeyError::UnsafeSessionId);
        }
        Ok(Self {
            project_key,
            session_id,
            subpath: None,
        })
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

    pub fn from_transcript_path(file_path: &Path, projects_dir: &Path) -> Result<Self, SessionKeyError> {
        if !file_path.is_absolute() || !projects_dir.is_absolute() {
            return Err(SessionKeyError::OutsideProjects);
        }
        let relative = file_path
            .strip_prefix(projects_dir)
            .map_err(|_| SessionKeyError::OutsideProjects)?;
        let mut parts = Vec::new();
        for component in relative.components() {
            let Component::Normal(part) = component else {
                return Err(SessionKeyError::OutsideProjects);
            };
            parts.push(part.to_str().ok_or(SessionKeyError::NotUnicode)?);
        }
        match parts.as_slice() {
            [project, transcript] => {
                let session = transcript
                    .strip_suffix(".jsonl")
                    .ok_or(SessionKeyError::UnrecognizedPath)?;
                Self::new(SessionProjectKey::new(*project)?, SessionId::from(session))
            }
            [project, session, rest @ ..] if rest.len() >= 2 => {
                let joined = rest.join("/");
                let subpath = joined.strip_suffix(".jsonl").unwrap_or(&joined);
                Ok(Self::new(SessionProjectKey::new(*project)?, SessionId::from(*session))?
                    .with_subpath(SessionSubpath::new(subpath)?))
            }
            _ => Err(SessionKeyError::UnrecognizedPath),
        }
    }
}
