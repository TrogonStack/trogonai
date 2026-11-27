//! A second, kernel-enforced filesystem layer behind the bubblewrap mounts.
//!
//! The mounts decide what the sandbox can see and write. Landlock restates
//! the write half of that decision as a ruleset on the payload process, so a
//! bind that was planned too wide still cannot be written through. The
//! ruleset only ever restricts: it handles write rights and grants them back
//! for exactly the roots that the mount plan makes writable, so a path the
//! mounts keep read-only stays read-only and a path the mounts hide stays
//! hidden whatever the ruleset says.
//!
//! Rules bind to descriptors opened inside the sandbox, never to path
//! strings, so a path swapped after the ruleset is built cannot redirect a
//! grant.
//!
//! The ruleset has to be built after the mounts exist, because Landlock
//! forbids mounting for a restricted process. The worker binary doubles as
//! the launcher (`--confine`): it is the first process in the sandbox, applies
//! the ruleset to itself and executes the payload.

use std::fmt;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use clap::Parser;
use landlock::{
    ABI, AccessFs, BitFlags, PathBeneath, PathFd, PathFdError, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus,
};
use nix::libc;
use trogon_sandbox_contract::{LiteralPath, LiteralPathError};
use trogon_sandbox_protocol::LiteralPathExt;

use super::mount_plan::{MountAccess, MountOp, MountPlan};

pub use super::constants::CONFINE_EXIT_CODE;
pub use super::constants::CONFINE_FLAG;
pub(super) use super::constants::GUEST_LAUNCHER_BINARY;
use super::constants::{DEV, DEV_SHM, PROC, TMP};

/// Rights outside of what the mounts decide: terminal ioctls and connecting
/// to unix sockets (which the seccomp filters already deny).
fn unhandled() -> BitFlags<AccessFs> {
    AccessFs::IoctlDev | AccessFs::ResolveUnix
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LandlockAbi(u8);

impl LandlockAbi {
    /// First ABI that handles every write right the ruleset relies on: file
    /// reparenting (2) and truncation (3).
    const FULL_WRITE_COVERAGE: Self = Self(3);

    pub fn new(version: u8) -> Option<Self> {
        (version >= 1).then_some(Self(version))
    }

    pub fn version(self) -> u8 {
        self.0
    }

    fn handled_access(self) -> BitFlags<AccessFs> {
        AccessFs::from_write(ABI::from(i32::from(self.0))) & !unhandled()
    }
}

impl fmt::Display for LandlockAbi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

/// What the running kernel lets the worker enforce as the second layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandlockSupport {
    Unavailable,
    Partial(LandlockAbi),
    Full(LandlockAbi),
}

impl LandlockSupport {
    pub fn detect() -> Self {
        const LANDLOCK_CREATE_RULESET_VERSION: libc::c_ulong = 1;
        let raw = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<libc::c_void>(),
                0 as libc::size_t,
                LANDLOCK_CREATE_RULESET_VERSION,
            )
        };
        Self::from_kernel_version(raw)
    }

    pub fn from_kernel_version(raw: libc::c_long) -> Self {
        let version = u8::try_from(raw.clamp(0, i64::from(u8::MAX))).unwrap_or(0);
        LandlockAbi::new(version).map_or(Self::Unavailable, |abi| {
            if abi >= LandlockAbi::FULL_WRITE_COVERAGE {
                Self::Full(abi)
            } else {
                Self::Partial(abi)
            }
        })
    }

    pub fn abi(self) -> Option<LandlockAbi> {
        match self {
            Self::Unavailable => None,
            Self::Partial(abi) | Self::Full(abi) => Some(abi),
        }
    }

    pub fn is_enforced(self) -> bool {
        self.abi().is_some()
    }

    pub fn log(self) {
        match self {
            Self::Unavailable => tracing::warn!(
                level = %self,
                "capability downgrade: the kernel has no Landlock, filesystem writes are confined by the mounts only"
            ),
            Self::Partial(abi) => tracing::warn!(
                level = %self,
                abi = %abi,
                "capability downgrade: the kernel Landlock ABI predates file reparenting and truncation control"
            ),
            Self::Full(abi) => tracing::info!(
                level = %self,
                abi = %abi,
                "filesystem writes are also confined by Landlock"
            ),
        }
    }
}

impl fmt::Display for LandlockSupport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("none"),
            Self::Partial(abi) => write!(f, "landlock-{abi}-partial"),
            Self::Full(abi) => write!(f, "landlock-{abi}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteScope {
    /// Every write right, beneath the root.
    Tree,
    /// Writing to files that already exist beneath the root, nothing that
    /// creates, removes or renames an entry.
    ExistingFiles,
}

impl WriteScope {
    fn access(self, handled: BitFlags<AccessFs>) -> BitFlags<AccessFs> {
        match self {
            Self::Tree => handled,
            Self::ExistingFiles => handled & (AccessFs::WriteFile | AccessFs::Truncate),
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Self::Tree => "--write",
            Self::ExistingFiles => "--write-existing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteGrant {
    pub root: LiteralPath,
    pub scope: WriteScope,
}

impl WriteGrant {
    pub fn tree(root: LiteralPath) -> Self {
        Self {
            root,
            scope: WriteScope::Tree,
        }
    }

    pub fn existing_files(root: LiteralPath) -> Self {
        Self {
            root,
            scope: WriteScope::ExistingFiles,
        }
    }
}

/// The roots the payload may write to, in guest paths.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WritePlan {
    grants: Vec<WriteGrant>,
}

impl WritePlan {
    pub fn new(grants: impl IntoIterator<Item = WriteGrant>) -> Self {
        let mut grants: Vec<WriteGrant> = grants.into_iter().collect();
        grants.sort_by(|a, b| a.root.as_str().cmp(b.root.as_str()));
        grants.dedup();
        let trees: Vec<LiteralPath> = grants
            .iter()
            .filter(|grant| grant.scope == WriteScope::Tree)
            .map(|grant| grant.root.clone())
            .collect();
        let covered = |grant: &WriteGrant| {
            trees
                .iter()
                .any(|tree| grant.root.is_within(tree) && (grant.root != *tree || grant.scope != WriteScope::Tree))
        };
        grants.retain(|grant| !covered(grant));
        Self { grants }
    }

    /// What the mounts of a lease leave writable, plus the device nodes and
    /// shared memory that `--dev` provides and the payload needs to run.
    pub fn from_mounts(plan: &MountPlan) -> Result<Self, LiteralPathError> {
        let mut writable: Vec<LiteralPath> = Vec::new();
        for op in &plan.ops {
            match op {
                MountOp::Bind { dest, access, .. } if *access == MountAccess::ReadWrite => {
                    writable.retain(|root| root != dest);
                    writable.push(dest.clone());
                }
                MountOp::Bind { dest, .. }
                | MountOp::EmptyDir { dest }
                | MountOp::FileStub { dest }
                | MountOp::Symlink { dest, .. }
                | MountOp::RemountReadOnly { dest } => writable.retain(|root| root != dest),
            }
        }
        let mut grants: Vec<WriteGrant> = writable.into_iter().map(WriteGrant::tree).collect();
        if plan.tmp_writable {
            grants.push(WriteGrant::tree(LiteralPath::new(TMP)?));
        }
        grants.push(WriteGrant::existing_files(LiteralPath::new(DEV)?));
        grants.push(WriteGrant::tree(LiteralPath::new(DEV_SHM)?));
        grants.push(WriteGrant::existing_files(LiteralPath::new(PROC)?));
        Ok(Self::new(grants))
    }

    pub fn grants(&self) -> &[WriteGrant] {
        &self.grants
    }

    fn args(&self) -> Vec<String> {
        self.grants
            .iter()
            .flat_map(|grant| [grant.scope.flag().to_string(), grant.root.as_str().to_string()])
            .collect()
    }
}

/// The ruleset the launcher applies before it executes the payload.
#[derive(Debug, Clone)]
pub struct Confinement {
    abi: LandlockAbi,
    writes: WritePlan,
}

impl Confinement {
    pub fn new(abi: LandlockAbi, writes: WritePlan) -> Self {
        Self { abi, writes }
    }

    pub fn writes(&self) -> &WritePlan {
        &self.writes
    }

    /// Runs `binary args` behind the launcher at `launcher`.
    pub(super) fn wrap_command(&self, launcher: &str, binary: &Path, args: &[String]) -> (PathBuf, Vec<String>) {
        let mut wrapped = vec![
            CONFINE_FLAG.to_string(),
            "--abi".to_string(),
            self.abi.version().to_string(),
        ];
        wrapped.extend(self.writes.args());
        wrapped.extend(["--".to_string(), binary.to_string_lossy().into_owned()]);
        wrapped.extend(args.iter().cloned());
        (PathBuf::from(launcher), wrapped)
    }
}

#[derive(Parser, Debug)]
#[command(name = "sandbox-subprocess-worker --confine")]
pub struct ConfineArgs {
    #[arg(long = "confine", required = true)]
    confine: bool,

    #[arg(long)]
    abi: u8,

    #[arg(long = "write")]
    write: Vec<PathBuf>,

    #[arg(long = "write-existing")]
    write_existing: Vec<PathBuf>,

    #[arg(last = true, required = true)]
    command: Vec<String>,
}

pub fn is_confine_invocation(args: &[std::ffi::OsString]) -> bool {
    args.get(1).is_some_and(|arg| arg == CONFINE_FLAG)
}

/// Runs inside the sandbox. Only returns on failure: the payload replaces
/// this process, and it never starts unless the ruleset is enforced.
pub fn run(args: ConfineArgs) -> anyhow::Result<std::convert::Infallible> {
    let abi = LandlockAbi::new(args.abi).context("the Landlock ABI must be at least 1")?;
    let roots = args
        .write
        .iter()
        .map(|path| (path, WriteScope::Tree))
        .chain(args.write_existing.iter().map(|path| (path, WriteScope::ExistingFiles)));
    restrict_self(abi, roots)?;

    let (binary, rest) = args.command.split_first().context("no command to run")?;
    let err = Command::new(binary).args(rest).exec();
    Err(err).with_context(|| format!("starting {binary}"))
}

/// A root that does not exist cannot be written to, so skipping it only
/// restricts further.
fn restrict_self<'a>(
    abi: LandlockAbi,
    roots: impl IntoIterator<Item = (&'a PathBuf, WriteScope)>,
) -> anyhow::Result<()> {
    let handled = abi.handled_access();
    let mut ruleset = Ruleset::default()
        .handle_access(handled)
        .context("declaring the handled Landlock rights")?
        .create()
        .context("creating the Landlock ruleset")?;
    for (root, scope) in roots {
        let fd = match PathFd::new(root) {
            Ok(fd) => fd,
            Err(PathFdError::OpenCall { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                continue;
            }
            Err(err) => {
                return Err(err).with_context(|| format!("opening {}", root.display()));
            }
        };
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, scope.access(handled)))
            .with_context(|| format!("granting writes beneath {}", root.display()))?;
    }
    let status = ruleset.restrict_self().context("enforcing the Landlock ruleset")?;
    anyhow::ensure!(
        status.ruleset != RulesetStatus::NotEnforced,
        "the Landlock ruleset was not enforced"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
