//! The default branch guess of the upstream bridge: what the repository around a directory says about the remote's
//! default branch, read straight from the repository files without running Git.
//!
//! The upstream reader refuses paths that reach a network share and follows symbolic links only when each target
//! passes the same checks; those checks live in the path guard.
//!
//! The upstream reader caches its answer per host and watches the repository files to refresh it. The cache only
//! saves work and never changes an answer, so this port reads the files on every call.

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use crate::bridge_gitcmd::GitProbeContext;
use crate::bridge_gitdir::{as_str, common_dir, find_git_dir};
use crate::bridge_paths::PathGuard;
use crate::session_prompt::js_trim;

mod constants;

use constants::{COMMON_FALLBACKS, FALLBACK, ORIGIN, REF_DEPTH, REFTABLE_MARKER};

/// The default branch of the repository around `directory`; `main` when nothing says otherwise, never empty.
pub(crate) async fn guess_default_branch(context: &GitProbeContext, directory: &Path) -> String {
    let Some(git_dir) = find_git_dir(&context.paths, directory) else {
        return FALLBACK.to_owned();
    };
    let repository = common_dir(&context.paths, &git_dir).unwrap_or(git_dir);
    if let Some(name) = origin_head(context, &repository).await
        && resolves(context, &repository, &format!("{ORIGIN}{name}"), REF_DEPTH).await
    {
        return name;
    }
    for name in COMMON_FALLBACKS {
        if resolves(context, &repository, &format!("{ORIGIN}{name}"), REF_DEPTH).await {
            return name.to_owned();
        }
    }
    FALLBACK.to_owned()
}

fn read_text(path: &Path) -> Option<String> {
    fs::read(path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// Whether the repository stores its references in the table format, which is read by asking Git.
fn is_reftable(paths: &PathGuard, repository: &Path) -> bool {
    let head = PathBuf::from(paths.join(as_str(repository), "HEAD"));
    paths.walks(as_str(&head), as_str(repository))
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
async fn origin_head(context: &GitProbeContext, repository: &Path) -> Option<String> {
    if is_reftable(&context.paths, repository) {
        let name = format!("{ORIGIN}HEAD");
        let output = context
            .run(
                repository,
                &["symbolic-ref", "-q", "--no-recurse", "--end-of-options", &name],
            )
            .await;
        let target = output.filter(|output| output.success).map(|output| output.stdout)?;
        let name = js_trim(&target).strip_prefix(ORIGIN)?;
        return valid_ref_name(name).then(|| name.to_owned());
    }
    let path = PathBuf::from(context.paths.join(as_str(repository), &format!("{ORIGIN}HEAD")));
    if !context.paths.walks(as_str(&path), as_str(repository)) {
        return None;
    }
    let text = read_text(&path)?;
    let target = js_trim(&text).strip_prefix("ref:").map(js_trim)?;
    let name = target.strip_prefix(ORIGIN)?;
    valid_ref_name(name).then(|| name.to_owned())
}

type Resolution<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

/// Whether `name` resolves to an object, in the table format, the loose files, the packed references or the common
/// directory.
fn resolves<'a>(context: &'a GitProbeContext, repository: &'a Path, name: &'a str, depth: u32) -> Resolution<'a> {
    Box::pin(async move {
        if depth == 0 {
            return false;
        }
        if is_reftable(&context.paths, repository) {
            if name != "HEAD" && !(name.starts_with("refs/") && valid_ref_name(name)) {
                return false;
            }
            let output = context
                .run(
                    repository,
                    &["show-ref", "--verify", "--hash", "--end-of-options", name],
                )
                .await;
            return output.is_some_and(|output| output.success && is_object_id(js_trim(&output.stdout)));
        }
        if read_ref(context, repository, name, depth).await {
            return true;
        }
        match common_dir(&context.paths, repository).filter(|common| common != repository) {
            Some(common) => read_ref(context, &common, name, depth).await,
            None => false,
        }
    })
}

async fn read_ref(context: &GitProbeContext, directory: &Path, name: &str, depth: u32) -> bool {
    let path = PathBuf::from(context.paths.join(as_str(directory), name));
    if !context.paths.walks(as_str(&path), as_str(directory)) {
        return false;
    }
    if let Some(body) = read_text(&path) {
        let text = js_trim(&body);
        return match text.strip_prefix("ref:").map(js_trim) {
            Some(target) => valid_ref_name(target) && resolves(context, directory, target, depth - 1).await,
            None => is_object_id(text),
        };
    }
    let packed = PathBuf::from(context.paths.join(as_str(directory), "packed-refs"));
    context.paths.walks(as_str(&packed), as_str(directory)) && packed_ref(&packed, name)
}

/// Whether the packed references list `name` with an object id.
fn packed_ref(packed: &Path, name: &str) -> bool {
    fs::read(packed).is_ok_and(|data| crate::bridge_packed::contains(&data, name))
}

#[cfg(test)]
#[path = "bridge_refs_tests.rs"]
mod tests;
