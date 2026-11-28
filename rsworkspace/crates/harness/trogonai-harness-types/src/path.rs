//! Lexical path algebra.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The longest chain of path components a resolved target may have.
///
/// A cap rather than a limit the platform happens to impose: the proof carried
/// past a permission decision is charged against a byte budget, and a path with
/// no bound on its component count has no bound on that charge either.
pub const MAX_FILE_TARGET_COMPONENTS: usize = 256;

/// Whether a path is absolute in the host-defined path namespace.
#[must_use]
pub fn is_absolute(path: &Path) -> bool {
    // Component guests carry POSIX paths, but std's unknown target requires
    // a drive prefix even though its path parser has no prefix syntax.
    if cfg!(target_arch = "wasm32") {
        path.has_root()
    } else {
        path.is_absolute()
    }
}

/// Where one component sits inside a path.
///
/// It lives here because the resolver produces spans and the mutation proof consumes them, and
/// neither of those crates may depend on the other.
///
/// The proof is one string plus offsets rather than a list of owned strings, so
/// that every part of it is provably a part of the same path. Independent
/// strings could drift apart; a span cannot name a path the target does not
/// contain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathSpan {
    /// First byte of the component.
    pub start: usize,
    /// One past the last byte of the component.
    pub end: usize,
}

/// Collapses `.`, `..`, and repeated separators without consulting the disk.
///
/// A `..` that would climb past the start is dropped rather than kept, matching
/// `normalizeRelativePathPartInto`, whose pop at the root is a no-op. Callers
/// anchor on an absolute base before normalizing, so that case only arises for
/// input that was trying to climb out of it.
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    let mut poppable = 0usize;

    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if poppable > 0 {
                    normalized.pop();
                    poppable -= 1;
                }
            }
            Component::Normal(part) => {
                normalized.push(part);
                poppable += 1;
            }
        }
    }

    normalized
}

/// Normalizes `relative` against `base`.
///
/// An absolute `relative` replaces `base` entirely, which is what
/// `std.fs.path.resolve` does with the same two arguments.
#[must_use]
pub fn resolve_against(base: &Path, relative: &Path) -> PathBuf {
    normalize(&base.join(relative))
}

/// `to` written as a path from `from`, climbing out of it where it has to.
///
/// Two paths that name the same place produce the empty path, which is that function's answer
/// too and not a failure: the caller decides whether "here" is worth a word.
///
/// This is the harsher of the two ways to say where a path sits. The other is
/// `WorkspaceRoot::relativize`, which hands back the absolute path rather than
/// climbing out of the root. Which one a caller wants depends on whether the
/// root it is relative to is on screen.
#[must_use]
pub fn relative(from: &Path, to: &Path) -> PathBuf {
    let from = normalize(from);
    let to = normalize(to);

    let mut source = from.components().peekable();
    let mut target = to.components().peekable();
    while let (Some(here), Some(there)) = (source.peek(), target.peek()) {
        if here != there {
            break;
        }
        source.next();
        target.next();
    }

    let mut out = PathBuf::new();
    for _ in source {
        out.push("..");
    }
    out.extend(target);
    out
}

/// Whether `candidate` is `root` or sits beneath it.
///
/// Compares whole components rather than bytes, so `/workspace-evil` is not inside `/workspace`
/// and a trailing separator on the root changes nothing.
///
/// `ensurePathInsideWorkspace` needs that guard because it is handed a bare string;
/// `trogonai-harness-workspace` moves it into the type that holds the root.
#[must_use]
pub fn path_inside(root: &Path, candidate: &Path) -> bool {
    if root == candidate {
        return true;
    }
    if root.as_os_str().is_empty() {
        return false;
    }
    candidate.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_below_the_one_it_is_measured_from_loses_the_shared_head() {
        assert_eq!(
            relative(Path::new("/w"), Path::new("/w/src/main.rs")),
            Path::new("src/main.rs")
        );
        assert_eq!(
            relative(Path::new("/w/"), Path::new("/w/./src//main.rs")),
            Path::new("src/main.rs")
        );
    }

    #[test]
    fn a_path_outside_the_one_it_is_measured_from_climbs_out_of_it() {
        assert_eq!(relative(Path::new("/w/a/b"), Path::new("/w/c")), Path::new("../../c"));
        assert_eq!(relative(Path::new("/w"), Path::new("/etc")), Path::new("../etc"));
    }

    #[test]
    fn the_same_place_measured_from_itself_is_no_path_at_all() {
        assert_eq!(relative(Path::new("/w"), Path::new("/w")), Path::new(""));
        assert_eq!(relative(Path::new("/w"), Path::new("/w/.")), Path::new(""));
    }

    #[test]
    fn dot_segments_and_repeated_separators_collapse() {
        assert_eq!(normalize(Path::new("/a/./b//c")), Path::new("/a/b/c"));
        assert_eq!(normalize(Path::new("/a/b/../c")), Path::new("/a/c"));
        assert_eq!(normalize(Path::new("/a/b/")), Path::new("/a/b"));
    }

    #[test]
    fn climbing_past_the_root_stays_at_the_root() {
        assert_eq!(normalize(Path::new("/../../a")), Path::new("/a"));
        assert_eq!(normalize(Path::new("/..")), Path::new("/"));
    }

    #[test]
    fn a_relative_climb_past_the_start_is_dropped() {
        assert_eq!(normalize(Path::new("a/../../b")), Path::new("b"));
    }

    #[test]
    fn an_absolute_relative_part_replaces_the_base() {
        assert_eq!(
            resolve_against(Path::new("/workspace"), Path::new("/etc/passwd")),
            Path::new("/etc/passwd")
        );
        assert_eq!(
            resolve_against(Path::new("/workspace"), Path::new("src/../lib.rs")),
            Path::new("/workspace/lib.rs")
        );
    }

    #[test]
    fn containment_preserves_exact_child_empty_root_and_prefix_semantics() {
        assert!(path_inside(Path::new("/workspace"), Path::new("/workspace")));
        assert!(path_inside(Path::new("/workspace"), Path::new("/workspace/file.txt")));
        assert!(path_inside(Path::new("/workspace/"), Path::new("/workspace/file.txt")));
        assert!(path_inside(Path::new(""), Path::new("")));
        assert!(!path_inside(Path::new(""), Path::new("/workspace")));
        assert!(!path_inside(
            Path::new("/workspace"),
            Path::new("/workspace-evil/file.txt")
        ));
    }
}
