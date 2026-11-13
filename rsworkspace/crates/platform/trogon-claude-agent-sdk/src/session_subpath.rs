/// A store subkey below a session, such as `subagents/agent-1`.
///
/// Subpaths come from external stores and become file paths during resume, so a subpath must stay inside the
/// session directory: it is relative, never empty, and has no `.` or `..` component on either separator. A subpath
/// listed during a TypeScript-dialect resume follows upstream's check instead: relative, never empty, no `..`
/// component, and its transcript file resolves inside the session directory, so `.` components can remain.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionSubpath(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("session subpath must be a relative path that stays inside the session directory")]
pub struct SessionSubpathError;

impl SessionSubpath {
    pub fn new(value: impl Into<String>) -> Result<Self, SessionSubpathError> {
        let value = value.into();
        if !is_safe(&value) {
            return Err(SessionSubpathError);
        }
        Ok(Self(value))
    }

    /// A subpath that already passed upstream TypeScript's `listSubkeys` check during resume.
    pub(crate) fn listed_by_typescript(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_safe(value: &str) -> bool {
    if value.is_empty() || value.starts_with(['/', '\\']) || value.contains('\0') || has_drive_prefix(value) {
        return false;
    }
    value.split(['/', '\\']).all(|part| {
        part != "."
            && part != ".."
            && (!cfg!(windows) || (!part.is_empty() && crate::session_path_component::is_safe(part)))
    })
}

fn has_drive_prefix(value: &str) -> bool {
    let mut chars = value.chars();
    matches!((chars.next(), chars.next()), (Some(drive), Some(':')) if drive.is_ascii_alphabetic())
}

impl std::fmt::Display for SessionSubpath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
