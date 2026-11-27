//! The filesystem rules of a Windows lease in the form the fence helper's `exec` subcommand takes.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context as _, bail};
use trogon_sandbox_contract::{FsPathEntry, FsPolicy, GlobPattern, LiteralPath};
use trogon_sandbox_protocol::{GlobMatcher, LiteralPathExt as _};
use trogon_sandbox_windows_fence::ExecRequest;
use trogon_sandbox_windows_fence::policy_path::{grants_beneath_denies_by, internal, native};

/// What a name on disk is, as far as the expansion of a pattern cares.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Kind {
    Directory,
    /// A file, or a link: a link is a match in its own right and is never
    /// descended into.
    Other,
}

/// The look at the disk that expanding the rules takes.
pub trait Disk {
    fn exists(&self, native_path: &str) -> bool;
    fn list(&self, native_path: &str) -> Vec<(String, Kind)>;
    /// The object a path leads to, with links, junctions, 8.3 names and case
    /// resolved, or the path itself when it cannot be looked at.
    fn resolve(&self, native_path: &str) -> String;
}

pub struct RealDisk;

impl Disk for RealDisk {
    fn exists(&self, native_path: &str) -> bool {
        Path::new(native_path).symlink_metadata().is_ok()
    }

    fn list(&self, native_path: &str) -> Vec<(String, Kind)> {
        let Ok(entries) = std::fs::read_dir(native_path) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let kind = match entry.file_type() {
                    Ok(kind) if kind.is_dir() => Kind::Directory,
                    _ => Kind::Other,
                };
                Some((name, kind))
            })
            .collect()
    }

    fn resolve(&self, native_path: &str) -> String {
        std::fs::canonicalize(native_path)
            .ok()
            .and_then(|real| real.to_str().and_then(internal))
            .and_then(|real| native(&real))
            .unwrap_or_else(|| native_path.to_string())
    }
}

/// Whether the entries name something to grant (a path that is not there is
/// dropped) or to deny (it is kept so the fence can hold the place).
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Mode {
    Grant,
    Deny,
}

fn literal_native(path: &LiteralPath) -> anyhow::Result<String> {
    native(path.as_str()).with_context(|| {
        format!(
            "{} is not a Windows path, so the fence cannot apply a rule to it",
            path.as_str()
        )
    })
}

fn walk(pattern: &GlobPattern, disk: &dyn Disk) -> anyhow::Result<Vec<String>> {
    let matcher = GlobMatcher::new(pattern);
    let base = matcher.static_base()?;
    let base_native = literal_native(&base)?;
    let mut out = Vec::new();
    if matcher.matches_ignoring_case(&base) && disk.exists(&base_native) {
        out.push(base_native.clone());
    }
    let mut visited = 0usize;
    let mut stack = vec![(base, base_native)];
    while let Some((lexical, host)) = stack.pop() {
        for (name, kind) in disk.list(&host) {
            visited += 1;
            if visited > MAX_GLOB_VISITS {
                bail!("the pattern {pattern} matches too much of the disk to be expanded");
            }
            let separator = if lexical.is_root() { "" } else { "/" };
            let Ok(child) = LiteralPath::new(format!("{}{separator}{name}", lexical.as_str())) else {
                continue;
            };
            let child_host = format!("{}\\{name}", host.trim_end_matches('\\'));
            if matcher.matches_ignoring_case(&child) {
                out.push(child_host);
            } else if kind == Kind::Directory {
                stack.push((child, child_host));
            }
        }
    }
    Ok(out)
}

/// The literal paths a list of entries stands
/// for, with a pattern replaced by what it matches on `disk` now.
pub fn expand(entries: &[FsPathEntry], mode: Mode, disk: &dyn Disk) -> anyhow::Result<Vec<String>> {
    let mut out = BTreeSet::new();
    let mut ordered = Vec::new();
    let mut keep = |path: String, ordered: &mut Vec<String>| {
        if out.insert(path.to_lowercase()) {
            ordered.push(path);
        }
    };
    for entry in entries {
        match entry {
            FsPathEntry::Literal { path, directory } => {
                let native = literal_native(path)?;
                if native.starts_with(r"\\") || disk.exists(&native) {
                    keep(native, &mut ordered);
                } else if mode == Mode::Deny {
                    let leaf = if directory.is_directory() && !native.ends_with('\\') {
                        format!("{native}\\")
                    } else {
                        native
                    };
                    keep(leaf, &mut ordered);
                }
            }
            FsPathEntry::Glob { pattern } => {
                for found in walk(pattern, disk)? {
                    keep(found, &mut ordered);
                }
            }
        }
    }
    Ok(ordered)
}

/// Adds the rules of `policy` to `request`: write
/// roots, `--deny-read`, `--deny-write`, and as `--deny-read-except` the
/// topmost grants beneath a read deny. Patterns are expanded against the disk
/// as `expand` does.
pub fn apply_fs_policy(request: &mut ExecRequest, policy: &FsPolicy) -> anyhow::Result<()> {
    apply_fs_policy_on(request, policy, &RealDisk)
}

pub fn apply_fs_policy_on(request: &mut ExecRequest, policy: &FsPolicy, disk: &dyn Disk) -> anyhow::Result<()> {
    let write_roots = expand(policy.allow_write(), Mode::Grant, disk)?;
    let mut deny_read = expand(policy.deny_read(), Mode::Deny, disk)?;
    let credential_deny = expand(policy.credential_deny_read(), Mode::Deny, disk)?;
    let deny_write = expand(policy.deny_write(), Mode::Deny, disk)?;
    let grants: Vec<String> = expand(policy.allow_read_within_deny(), Mode::Grant, disk)?
        .into_iter()
        .chain(write_roots.iter().cloned())
        .collect();
    request.deny_read_except = grants_beneath_denies_by(&grants, &deny_read, |path| disk.resolve(path));
    request.write_roots.extend(write_roots);
    for path in credential_deny {
        if !deny_read.contains(&path) {
            deny_read.push(path);
        }
    }
    request.deny_read = deny_read;
    request.deny_write = deny_write;
    Ok(())
}

mod constants;
#[cfg(test)]
mod tests;
use constants::MAX_GLOB_VISITS;
