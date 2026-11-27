//! The paths that stay read-only at the working directory, whatever the
//! [`FsPolicy`](trogon_sandbox_contract::FsPolicy) says.
//!
//! Each entry is a way for sandboxed code to run later outside
//! the sandbox: shell start-up files, git hooks and config, editor and tool
//! configuration that the user's own tools execute or trust.

#[cfg(target_os = "macos")]
pub use super::constants::GIT_HOOKS;
#[cfg(target_os = "linux")]
pub use super::constants::ROOT_PATHS;
pub use super::constants::{
    DANGEROUS_DIRECTORIES, DANGEROUS_FILES, DOT_AGENT, DOT_AGENT_PROTECTED, DOT_GIT, GIT_DIR_PROTECTED,
    GIT_DIR_PROTECTED_WITHOUT_CONFIG,
};

pub fn git_dir_protected(allow_git_config: bool) -> &'static [&'static str] {
    if allow_git_config {
        &GIT_DIR_PROTECTED_WITHOUT_CONFIG
    } else {
        &GIT_DIR_PROTECTED
    }
}

pub fn is_dangerous_file(name: &str) -> bool {
    DANGEROUS_FILES.iter().any(|file| name.eq_ignore_ascii_case(file))
}

pub fn is_dangerous_directory(name: &str) -> bool {
    DANGEROUS_DIRECTORIES.iter().any(|dir| name.eq_ignore_ascii_case(dir))
}

pub fn is_dot_git(name: &str) -> bool {
    name.eq_ignore_ascii_case(DOT_GIT)
}

pub fn is_dot_agent(name: &str) -> bool {
    name.eq_ignore_ascii_case(DOT_AGENT)
}

pub fn is_dot_agent_protected(name: &str) -> bool {
    DOT_AGENT_PROTECTED.iter().any(|entry| name.eq_ignore_ascii_case(entry))
}

/// The directory a `.git` file points at (`gitdir: <path>`), or `None` when
/// the content is not a pointer.
pub fn gitdir_pointer(content: &[u8]) -> Option<&str> {
    let line = std::str::from_utf8(content).ok()?.lines().next()?;
    let target = line.strip_prefix("gitdir:")?.trim();
    (!target.is_empty() && !target.contains('\0')).then_some(target)
}

/// The shared git directory a linked worktree's git directory names in its
/// `commondir` file.
pub fn commondir_pointer(content: &[u8]) -> Option<&str> {
    let target = std::str::from_utf8(content).ok()?.lines().next()?.trim();
    (!target.is_empty() && !target.contains('\0')).then_some(target)
}

#[cfg(test)]
mod tests;
