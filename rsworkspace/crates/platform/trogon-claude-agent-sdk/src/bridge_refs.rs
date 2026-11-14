//! The default branch guess of the upstream bridge: what the repository around a directory says about the remote's
//! default branch, read straight from the repository files without running Git.
//!
//! The upstream reader refuses paths that reach a network share and follows symbolic links only when each target
//! passes the same checks; the checks below are the POSIX ones.
//!
//! The upstream reader caches its answer per host and watches the repository files to refresh it. The cache only
//! saves work and never changes an answer, so this port reads the files on every call.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::bridge_js::js_trim;
use crate::bridge_unicode::js_lowercase;

const FALLBACK: &str = "main";
const COMMON_FALLBACKS: [&str; 2] = ["main", "master"];
const READ_LIMIT: u64 = 65536;
const LINK_DEPTH: u32 = 40;
const REF_DEPTH: u32 = 5;
const ORIGIN: &str = "refs/remotes/origin/";
const REFTABLE_MARKER: &str = "refs/heads/.invalid";

/// The default branch of the repository around `directory`; `main` when nothing says otherwise, never empty.
pub(crate) fn guess_default_branch(directory: &Path) -> String {
    let Some(git_dir) = find_git_dir(directory) else {
        return FALLBACK.to_owned();
    };
    let repository = common_dir(&git_dir).unwrap_or(git_dir);
    if is_reftable(&repository) {
        return FALLBACK.to_owned();
    }
    let advertised = origin_head(&repository);
    if let Some(name) = advertised.filter(|name| resolves(&repository, &format!("{ORIGIN}{name}"), REF_DEPTH)) {
        return name;
    }
    COMMON_FALLBACKS
        .iter()
        .find(|name| resolves(&repository, &format!("{ORIGIN}{name}"), REF_DEPTH))
        .map_or_else(|| FALLBACK.to_owned(), |name| (*name).to_owned())
}

/// `path.resolve(base, relative)`: an absolute `relative` wins, `.` and `..` are folded out.
fn resolve(base: &Path, relative: &str) -> PathBuf {
    let mut resolved = PathBuf::new();
    for component in base.join(relative).components() {
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    resolved
}

fn components_of(path: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other.to_owned()),
        }
    }
    parts
}

fn dot_special(first: &str) -> bool {
    matches!(
        js_lowercase(first).as_str(),
        ".vol" | ".file" | ".nofollow" | ".resolve"
    )
}

/// An absolute path on a network share or one of the special mount roots.
fn special_location(path: &str, exactly_net: bool) -> bool {
    if !path.starts_with('/') {
        return false;
    }
    let parts = components_of(path);
    let Some(first) = parts.first() else {
        return false;
    };
    js_lowercase(first) == "network"
        || dot_special(first)
        || (exactly_net && parts.len() == 1 && js_lowercase(first) == "net")
}

/// Whether text that names a path (a link target, a `gitdir:` value) is refused, relative to `base`.
fn text_unsafe(text: &str, base: &Path) -> bool {
    let resolved = resolve(base, text);
    [text, resolved.to_str().unwrap_or_default()].iter().any(|candidate| {
        candidate.starts_with("//")
            || candidate.starts_with("\\\\")
            || candidate.starts_with("\\??\\")
            || special_location(candidate, true)
    })
}

/// Where `text` leads from `base`, when neither the text nor any step of the way is refused.
fn reach(base: &Path, text: &str) -> Option<PathBuf> {
    if text_unsafe(text, base) {
        return None;
    }
    let joined = base.join(text);
    let resolved = resolve(base, text);
    (walk_safe(&joined, LINK_DEPTH) && (resolved == joined || walk_safe(&resolved, LINK_DEPTH))).then_some(resolved)
}

/// Follows `path` one component at a time the way the upstream reader does, and says whether every step is allowed.
fn walk_safe(path: &Path, depth: u32) -> bool {
    if depth == 0 {
        return false;
    }
    let parts: Vec<Component<'_>> = path.components().collect();
    let mut current = PathBuf::new();
    for (index, component) in parts.iter().enumerate() {
        match component {
            Component::ParentDir => {
                current.pop();
                continue;
            }
            Component::CurDir => continue,
            other => current.push(other),
        }
        if special_location(current.to_str().unwrap_or_default(), false) {
            return false;
        }
        let kind = match fs::symlink_metadata(&current) {
            Ok(meta) => meta.file_type(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return false,
        };
        if kind.is_symlink() {
            let Some(target) = fs::read_link(&current)
                .ok()
                .and_then(|target| target.to_str().map(str::to_owned))
                .filter(|target| !target.contains('\0'))
            else {
                return false;
            };
            let parent = current.parent().unwrap_or(&current).to_path_buf();
            if text_unsafe(&target, &parent) {
                return false;
            }
            let mut next = resolve(&parent, &target);
            next.extend(parts[index + 1..].iter().map(|part| part.as_os_str()));
            return walk_safe(&next, depth - 1);
        }
        if !kind.is_dir() && !kind.is_file() {
            return false;
        }
    }
    true
}

fn read_text(path: &Path) -> Option<String> {
    fs::read(path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Reads a small regular file, giving up on anything larger than the limit.
fn read_limited(path: &Path) -> Option<String> {
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

/// The `.git` directory of the nearest repository around `directory`, following a `gitdir:` file.
fn find_git_dir(directory: &Path) -> Option<PathBuf> {
    let start = resolve(directory, ".");
    let root = start
        .ancestors()
        .find(|candidate| fs::symlink_metadata(candidate.join(".git")).is_ok())?;
    let dot_git = root.join(".git");
    if !fs::metadata(&dot_git).ok()?.is_file() {
        return Some(dot_git);
    }
    let content = read_limited(&dot_git)?;
    let Some(target) = js_trim(&content).strip_prefix("gitdir:").map(js_trim) else {
        return Some(dot_git);
    };
    reach(root, target)
}

/// The directory a linked worktree shares its references with, when it names one.
fn common_dir(git_dir: &Path) -> Option<PathBuf> {
    let file = git_dir.join("commondir");
    if !walk_safe(&file, LINK_DEPTH) {
        return None;
    }
    if let Ok(meta) = fs::symlink_metadata(&file)
        && !(meta.is_file() || meta.is_dir())
    {
        return None;
    }
    let content = read_limited(&file)?;
    reach(git_dir, js_trim(&content))
}

/// Whether the repository stores its references in the table format, which this reader does not open.
fn is_reftable(repository: &Path) -> bool {
    let head = repository.join("HEAD");
    walk_safe(&head, LINK_DEPTH)
        && read_text(&head)
            .is_some_and(|text| js_trim(&text).strip_prefix("ref:").map(js_trim) == Some(REFTABLE_MARKER))
}

/// A reference name that cannot escape the references directory or smuggle in an option.
fn valid_ref_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '/'])
        && !name.contains("..")
        && !name.split('/').any(|part| part.is_empty() || part == ".")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '+' | '@' | '-'))
}

fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The branch `refs/remotes/origin/HEAD` points at.
fn origin_head(repository: &Path) -> Option<String> {
    let path = repository.join(format!("{ORIGIN}HEAD"));
    if !walk_safe(&path, LINK_DEPTH) {
        return None;
    }
    let text = read_text(&path)?;
    let target = js_trim(&text).strip_prefix("ref:").map(js_trim)?;
    let name = target.strip_prefix(ORIGIN)?;
    valid_ref_name(name).then(|| name.to_owned())
}

/// Whether `name` resolves to an object, in the loose files, the packed references or the common directory.
fn resolves(repository: &Path, name: &str, depth: u32) -> bool {
    if depth == 0 {
        return false;
    }
    if read_ref(repository, name, depth) {
        return true;
    }
    common_dir(repository)
        .filter(|common| common != repository)
        .is_some_and(|common| read_ref(&common, name, depth))
}

fn read_ref(directory: &Path, name: &str, depth: u32) -> bool {
    let path = directory.join(name);
    if !walk_safe(&path, LINK_DEPTH) {
        return false;
    }
    if let Some(text) = read_text(&path) {
        let text = js_trim(&text);
        return match text.strip_prefix("ref:").map(js_trim) {
            Some(target) => valid_ref_name(target) && resolves(directory, target, depth - 1),
            None => is_object_id(text),
        };
    }
    let packed = directory.join("packed-refs");
    walk_safe(&packed, LINK_DEPTH) && packed_ref(&packed, name)
}

/// Whether the packed references list `name` with an object id.
fn packed_ref(packed: &Path, name: &str) -> bool {
    fs::read(packed).is_ok_and(|data| crate::bridge_packed::contains(&data, name))
}

#[cfg(test)]
#[path = "bridge_refs_tests.rs"]
mod tests;
