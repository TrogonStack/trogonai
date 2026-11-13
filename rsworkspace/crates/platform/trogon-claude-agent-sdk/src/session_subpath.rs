/// A store subkey below a session, such as `subagents/agent-1`.
///
/// [`SessionSubpath::new`] takes any text, as upstream does: a store key is whatever the store holds. Upstream checks
/// a subpath only where it becomes a file during resume, and the two SDKs check it differently, so that guard lives
/// at the resume site of each dialect. [`SessionSubpath::checked`] is the opt-in constructor for callers that want
/// Rust's own rule: relative, never empty, no `.` or `..` component on either separator, no drive prefix or NUL.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionSubpath(String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("session subpath must be a relative path that stays inside the session directory")]
pub struct SessionSubpathError;

impl SessionSubpath {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// A subpath that stays inside the session directory when joined to it.
    pub fn checked(value: impl Into<String>) -> Result<Self, SessionSubpathError> {
        let value = value.into();
        if !is_safe(&value) {
            return Err(SessionSubpathError);
        }
        Ok(Self(value))
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
