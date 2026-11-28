//! The policy as the lists a backend enforces: what is read, what is written
//! and which hosts the network names.

use std::path::{Component, Path, PathBuf};

use crate::constants::HOME_CONVENIENCE_WRITE_DIRS;
#[cfg(windows)]
use crate::constants::{DEFAULT_WRITE_DEVICES, DEFAULT_WRITE_DIRS};
use trogon_sandbox_contract::DirectoryMarker;
use trogon_sandbox_contract::FsPathEntry;
use trogon_sandbox_contract::FsPolicy;
use trogon_sandbox_contract::LiteralPath;
use trogon_sandbox_contract::LiteralPathError;
use trogon_sandbox_contract::NetworkPolicy;
use trogon_sandbox_protocol::{CompiledFsEntry, LiteralPathExt};
#[cfg(unix)]
use trogon_sandbox_subprocess_worker::isolation::default_writes::{DEFAULT_WRITE_DEVICES, DEFAULT_WRITE_DIRS};
use trogon_std::env::{ReadEnv, SystemEnv};

/// Paths a read deny covers and the allows that open parts of it again.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct FsReadRestriction {
    pub deny_only: Vec<FsPathEntry>,
    pub allow_within_deny: Vec<FsPathEntry>,
}

/// Paths a command may write and the denies cut out of them. The default
/// write devices are part of `allow_only`.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct FsWriteRestriction {
    pub allow_only: Vec<FsPathEntry>,
    pub deny_within_allow: Vec<FsPathEntry>,
}

impl FsWriteRestriction {
    /// Every path the restriction allows writing, whichever list it came in
    /// on: what a caller asks to judge whether a command may write somewhere.
    pub fn roots(&self) -> &[FsPathEntry] {
        &self.allow_only
    }
}

/// Hosts the network policy names, as it spells them.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct NetworkRestriction {
    pub allowed_hosts: Vec<String>,
    pub denied_hosts: Vec<String>,
}

impl FsReadRestriction {
    pub fn of(policy: &FsPolicy) -> Self {
        Self {
            deny_only: policy.deny_read().to_vec(),
            allow_within_deny: policy.allow_read_within_deny().to_vec(),
        }
    }
}

impl FsWriteRestriction {
    pub fn of(policy: &FsPolicy) -> Self {
        Self {
            allow_only: default_write_entries(policy)
                .chain(policy.allow_write().iter().cloned())
                .collect(),
            deny_within_allow: policy.deny_write().to_vec(),
        }
    }

    /// What a disabled filesystem policy reports: the whole filesystem is
    /// writable.
    pub fn everywhere() -> Self {
        Self {
            allow_only: literal_entry("/").into_iter().collect(),
            deny_within_allow: Vec::new(),
        }
    }

    /// The restriction of a runtime with no filesystem policy.
    pub fn defaults() -> Self {
        Self::of(&FsPolicy::default())
    }
}

impl NetworkRestriction {
    pub fn of(policy: &NetworkPolicy) -> Self {
        match policy {
            NetworkPolicy::AllowAll | NetworkPolicy::DenyAll => Self::default(),
            NetworkPolicy::UserDefined {
                rules, denied_domains, ..
            } => Self {
                allowed_hosts: rules.iter().map(|rule| rule.domain.to_string()).collect(),
                denied_hosts: denied_domains.iter().map(ToString::to_string).collect(),
            },
        }
    }
}

/// A literal path entry for `path`.
pub(crate) fn literal_entry(path: &str) -> Result<FsPathEntry, LiteralPathError> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(path)?,
        directory: DirectoryMarker::Unmarked,
    })
}

/// The write deny for the credential store directory. A directory that cannot
/// be resolved or named as a literal path fails the launch, because skipping
/// the deny would leave the store writable from the sandbox.
#[cfg(unix)]
pub(crate) fn store_deny_entry(dir: &Path) -> Result<FsPathEntry, crate::error::RuntimeError> {
    let resolved = dir.canonicalize().map_err(|err| {
        crate::error::RuntimeError::Invalid(anyhow::anyhow!(
            "the credential store directory {} cannot be resolved, so it cannot be denied to the sandbox: {err}",
            dir.display()
        ))
    })?;
    literal_entry(&resolved.to_string_lossy()).map_err(|err| {
        crate::error::RuntimeError::Invalid(anyhow::anyhow!(
            "the credential store directory {} is not a valid deny path: {err}",
            resolved.display()
        ))
    })
}

/// The paths a command may always write: its standard streams and terminal,
/// the directory handed out as `TMPDIR` and the home directory conveniences.
pub fn default_write_paths() -> Vec<PathBuf> {
    default_write_paths_under(&FsPolicy::default())
}

/// [`default_write_paths`] for a policy: a home directory convenience is left
/// out when a read deny covers it, unless an allow of a literal path re-opens
/// it, and when a credential deny covers it, whatever the allows say.
pub fn default_write_paths_under(policy: &FsPolicy) -> Vec<PathBuf> {
    default_write_entries(policy)
        .filter_map(|entry| match entry {
            FsPathEntry::Literal { path, .. } => Some(PathBuf::from(path.as_str())),
            FsPathEntry::Glob { .. } => None,
        })
        .collect()
}

pub(crate) fn default_write_dir_entries(policy: &FsPolicy) -> impl Iterator<Item = FsPathEntry> {
    default_write_dir_entries_in(policy, &SystemEnv)
}

fn default_write_dir_entries_in(policy: &FsPolicy, env: &impl ReadEnv) -> impl Iterator<Item = FsPathEntry> {
    let dirs = DEFAULT_WRITE_DIRS.iter().filter_map(|path| literal_entry(path).ok());
    let home = env.var_os("HOME").and_then(|home| home.into_string().ok());
    dirs.chain(home_convenience_entries(policy, home))
}

fn default_write_entries(policy: &FsPolicy) -> impl Iterator<Item = FsPathEntry> {
    DEFAULT_WRITE_DEVICES
        .iter()
        .filter_map(|path| literal_entry(path).ok())
        .chain(default_write_dir_entries(policy))
}

fn home_convenience_entries(policy: &FsPolicy, home: Option<String>) -> Vec<FsPathEntry> {
    let Some(home) = home else {
        return Vec::new();
    };
    let canonical = std::fs::canonicalize(&home)
        .ok()
        .and_then(|path| path.to_str().map(str::to_string));
    let second = canonical.filter(|canonical| *canonical != home);
    let homes: Vec<String> = std::iter::once(home).chain(second).collect();
    HOME_CONVENIENCE_WRITE_DIRS
        .iter()
        .filter_map(|relative| {
            let spellings: Vec<LiteralPath> = homes
                .iter()
                .filter_map(|home| LiteralPath::new(format!("{}/{relative}", home.trim_end_matches('/'))).ok())
                .collect();
            let first = spellings.first()?.clone();
            let kept = left_open(policy.deny_read(), policy.allow_read_within_deny(), &spellings)
                && left_open(policy.credential_deny_read(), &[], &spellings);
            kept.then_some(FsPathEntry::Literal {
                path: first,
                directory: DirectoryMarker::Unmarked,
            })
        })
        .collect()
}

/// Whether no deny covers the directory, or every deny that does is opened
/// again by an allow of a literal path that it covers and that holds the
/// directory. A glob allow re-opens nothing.
fn left_open(denies: &[FsPathEntry], allows: &[FsPathEntry], spellings: &[LiteralPath]) -> bool {
    denies
        .iter()
        .map(CompiledFsEntry::new)
        .filter(|deny| spellings.iter().any(|spelling| deny.covers(spelling)))
        .all(|deny| {
            allows.iter().any(|allow| match allow {
                FsPathEntry::Literal { path, .. } => {
                    deny.covers(path) && spellings.iter().any(|spelling| spelling.is_within(path))
                }
                FsPathEntry::Glob { .. } => false,
            })
        })
}

/// `path` with every symbolic link resolved, after folding `.`, `..` and
/// repeated separators in the text, so `link/..` is the directory that holds
/// the link. A relative path is read from the working
/// directory.
pub fn real_path_of(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(fold(path.as_ref())?)
}

fn fold(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut folded = PathBuf::from("/");
    for component in absolute.components() {
        match component {
            Component::Normal(name) => folded.push(name),
            Component::ParentDir => {
                folded.pop();
            }
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    Ok(folded)
}

#[cfg(test)]
mod tests;
