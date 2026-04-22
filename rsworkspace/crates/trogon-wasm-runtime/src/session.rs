use std::path::{Path, PathBuf};

/// Per-session state: the isolated sandbox directory for this session.
pub struct WasmSession {
    /// Absolute path to this session's sandbox root.
    pub dir: PathBuf,
    /// Timestamp of the last activity on this session, used for idle expiry.
    pub last_activity: std::time::Instant,
}

impl WasmSession {
    pub fn new(dir: PathBuf, now: std::time::Instant) -> Self {
        Self {
            dir,
            last_activity: now,
        }
    }

    /// Resolves a path from the agent relative to the session sandbox.
    ///
    /// Any path traversal that escapes the sandbox root is rejected.
    pub fn resolve_path(&self, agent_path: &Path) -> Option<PathBuf> {
        // Strip leading `/` so paths like `/workspace/file.txt` map into the sandbox.
        let relative = agent_path.strip_prefix("/").unwrap_or(agent_path);

        let resolved = self.dir.join(relative);

        // Canonicalize the prefix check without requiring the file to exist:
        // normalize away `..` components by hand.
        let normalized = normalize_path(&resolved);
        if normalized.starts_with(&self.dir) {
            Some(normalized)
        } else {
            None
        }
    }

    /// Returns the effective working directory for a terminal command.
    ///
    /// If the agent provided a `cwd`, it is resolved inside the sandbox.
    /// Falls back to the session root if unset or out-of-bounds.
    pub fn terminal_cwd(&self, requested_cwd: Option<&Path>) -> PathBuf {
        requested_cwd
            .and_then(|p| self.resolve_path(p))
            .unwrap_or_else(|| self.dir.clone())
    }
}

/// Normalizes path components without requiring the path to exist on disk.
/// Resolves `.` and `..` lexically.
fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(name) => out.push(name),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(dir: &str) -> WasmSession {
        WasmSession::new(PathBuf::from(dir), std::time::Instant::now())
    }

    #[test]
    fn resolve_relative_path() {
        let s = session("/sandbox/abc");
        let p = s.resolve_path(Path::new("workspace/file.txt")).unwrap();
        assert_eq!(p, PathBuf::from("/sandbox/abc/workspace/file.txt"));
    }

    #[test]
    fn resolve_absolute_path_strips_leading_slash() {
        let s = session("/sandbox/abc");
        let p = s.resolve_path(Path::new("/workspace/file.txt")).unwrap();
        assert_eq!(p, PathBuf::from("/sandbox/abc/workspace/file.txt"));
    }

    #[test]
    fn path_traversal_rejected() {
        let s = session("/sandbox/abc");
        assert!(s.resolve_path(Path::new("../../etc/passwd")).is_none());
    }

    #[test]
    fn absolute_traversal_rejected() {
        let s = session("/sandbox/abc");
        assert!(s.resolve_path(Path::new("/../../etc/passwd")).is_none());
    }

    #[test]
    fn nested_path_ok() {
        let s = session("/sandbox/abc");
        let p = s.resolve_path(Path::new("a/b/../c")).unwrap();
        assert_eq!(p, PathBuf::from("/sandbox/abc/a/c"));
    }
}
