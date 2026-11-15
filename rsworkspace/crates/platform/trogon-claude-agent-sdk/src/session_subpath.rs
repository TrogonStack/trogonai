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
        if !is_safe(&value, cfg!(windows)) {
            return Err(SessionSubpathError);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Upstream Python's resume guard, without the file-system resolution it ends with: not empty, no leading separator,
/// no NUL, no drive (any character before a colon, as `ntpath.splitdrive` reads it, on every host) and no `.` or `..`
/// component on either separator.
pub(crate) fn python_accepts(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with(['/', '\\'])
        && !value.contains('\0')
        && value.chars().nth(1) != Some(':')
        && value.split(['/', '\\']).all(|part| part != "." && part != "..")
}

/// The [`SessionSubpath::checked`] rule, which is Rust's own and follows the platform. A drive prefix is a letter and
/// a colon on POSIX and, as `ntpath.splitdrive` reads it, any character and a colon on Windows. Windows also rejects
/// empty components and names it cannot create as files.
pub(crate) fn is_safe(value: &str, windows: bool) -> bool {
    let drive = if windows {
        value.chars().nth(1) == Some(':')
    } else {
        let mut chars = value.chars();
        matches!((chars.next(), chars.next()), (Some(drive), Some(':')) if drive.is_ascii_alphabetic())
    };
    if drive || value.is_empty() || value.starts_with(['/', '\\']) || value.contains('\0') {
        return false;
    }
    value.split(['/', '\\']).all(|part| {
        part != "." && part != ".." && (!windows || (!part.is_empty() && crate::session_path_component::is_safe(part)))
    })
}

impl std::fmt::Display for SessionSubpath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
