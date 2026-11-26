//! Where the Git directory of a repository is, read straight from the repository files without running Git.
//!
//! The default branch guess and the gated Git runner both start from these answers, so they live below both.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::bridge_paths::PathGuard;
use crate::bridge_platform::EntryKind;
use crate::session_prompt::js_trim;

mod constants;

use constants::READ_LIMIT;

/// Reads a small regular file, giving up on anything larger than the limit.
pub(crate) fn read_limited(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(READ_LIMIT + 1).read_to_end(&mut bytes).ok()?;
    u64::try_from(bytes.len())
        .is_ok_and(|length| length <= READ_LIMIT)
        .then(|| String::from_utf8_lossy(&bytes).into_owned())
}

pub(crate) fn as_str(path: &Path) -> &str {
    path.to_str().unwrap_or_default()
}

/// The `.git` directory of the nearest repository around `directory`, following a `gitdir:` file.
pub(crate) fn find_git_dir(paths: &PathGuard, directory: &Path) -> Option<PathBuf> {
    let root = paths.find_root(as_str(directory))?;
    let dot_git = PathBuf::from(paths.join(&root, ".git"));
    match paths.followed_kind(as_str(&dot_git)) {
        EntryKind::File => {}
        EntryKind::Absent | EntryKind::Other => return None,
        _ => return Some(dot_git),
    }
    let content = read_limited(&dot_git)?;
    let Some(target) = js_trim(&content).strip_prefix("gitdir:").map(js_trim) else {
        return Some(dot_git);
    };
    paths.reach(&root, target).map(PathBuf::from)
}

/// The directory a linked worktree shares its references with, when it names one.
pub(crate) fn common_dir(paths: &PathGuard, git_dir: &Path) -> Option<PathBuf> {
    let file = PathBuf::from(paths.join(as_str(git_dir), "commondir"));
    if !paths.walks(as_str(&file), as_str(git_dir))
        || matches!(
            paths.entry_kind(as_str(&file)),
            EntryKind::Symlink | EntryKind::Special | EntryKind::Other
        )
    {
        return None;
    }
    let content = read_limited(&file)?;
    paths.reach(as_str(git_dir), js_trim(&content)).map(PathBuf::from)
}
