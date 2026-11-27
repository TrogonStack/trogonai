//! Turns an [`FsPolicy`] into the mounts that make a bubblewrap sandbox
//! enforce it.
//!
//! The planner is pure: it reads the host through [`HostTree`] and returns a
//! [`MountPlan`]. Nothing is mounted here, so every decision is testable
//! without bubblewrap.
//!
//! The plan also defends against a host path changing between planning and
//! mounting: each bind source is opened without following links and checked
//! against what the plan saw.
//!
//! # Model
//!
//! - Every base mount (`/usr`, the system directories, the workspace, extra
//!   binds) is described by a [`GuestLayout`]. With a policy present they all
//!   start read-only: a mount is read-write only where `allow_write` grants it
//!   and the layout allowed writing there to begin with.
//! - Policy paths are resolved inside the guest namespace. A symlink on the
//!   way is followed once per component with a bounded number of hops, and an
//!   absolute link target is read in the guest, never on the host.
//! - `deny_read` and `deny_write` follow links and protect the resolved
//!   target. `allow_write` that traverses a link is not honoured and is
//!   reported in [`MountPlan::suppressed`], so a write grant can never be
//!   steered outside the path it names.
//! - An `allow_read_within_deny` literal that traverses a link is restored at
//!   its own name, read-only, from the vetted target. It is dropped, and
//!   reported, when a read deny that mounts something sits at, inside or
//!   around the target without also covering the name, since the target's own
//!   path would then serve what the deny hides.
//! - A denied directory becomes an empty tmpfs, a denied file becomes a
//!   read-only bind of `/dev/null`. A path that is allowed again inside a deny
//!   is bound back over the stub, unless the deny is more specific than the
//!   allow: a deny strictly inside an allowed literal path, or a pattern that
//!   no allow was written to open (see `FsPolicy::read_exceptions`), stays
//!   denied.
//! - There is exactly one mount per destination, ordered by depth and path, so
//!   the plan does not depend on the order of the policy entries.
//! - A protected path inside a writable region pins each directory above it
//!   with a self bind, so the sandbox cannot rename or remove an ancestor to
//!   walk around the protection (the rename fails with EBUSY). In the host
//!   view the pins are buried: they go in
//!   beneath the write root bind, or, under a `/` write root, after it and
//!   under a second bind of the top-level directory, so a rename or link
//!   across a pinned directory stays on one mount. Elsewhere they stay
//!   visible mounts.
//! - A denied path that does not exist yet gets a guard stub at its first
//!   missing component when that parent is writable. The mount point bubblewrap
//!   creates on the host is listed in [`MountPlan::placeholders`].
//! - The mandatory list (see `mandatory.rs`) is added to the `deny_write`
//!   targets for the working directory when it lies inside a write grant,
//!   after the policy is read, so no `FsPolicy` field can lift it. The
//!   paths at the top of the working directory are guarded even when absent;
//!   below it the tree is walked down to the search depth (default 3,
//!   bounded also by [`MAX_MANDATORY_VISITS`]) and each occurrence of
//!   a protected name is bound read-only, so files created later inside a
//!   protected directory are covered. A working directory outside every write
//!   grant has nothing to protect. Without a working directory the outermost
//!   writable roots are used instead. A missing `.git` is not guarded, because
//!   a mount there would stop `git init`. A `.git` file is bound read-only and
//!   the git directory it names (and its `commondir`) is protected when the
//!   guest can reach it; a pointer the guest cannot resolve leaves the sandbox
//!   nothing to write. A mandatory path reached through a symlink is resolved
//!   and its target is protected, as the resolved path is the object a bind
//!   can stop the sandbox from changing.
//!
//! # Limits that are part of the contract
//!
//! - Patterns are expanded against the tree when the plan is built. The walk
//!   follows a link to a directory and denies a match where it really is,
//!   but never into the root,
//!   an ancestor of the directory it is in, or a directory already on its path;
//!   a match that resolves to the root denies the directory holding the link.
//!   A carve-out inside a denied target is honoured whichever spelling reached
//!   it. A file
//!   created later that matches a `deny_read` pattern is the sandbox's own
//!   file and exposes no host data, and one that matches an `allow_write`
//!   pattern stays read-only (it fails closed). A `deny_write` pattern cannot
//!   be enforced on files that do not exist yet, so it is refused with
//!   [`FsPlanError::UnenforceableWriteGlob`] whenever it could overlap a
//!   writable grant.
//! - `/proc`, `/dev` and `/tmp` are provided by the sandbox. An entry inside
//!   them is refused, except `allow_write` and `deny_write` naming exactly
//!   `/tmp`, which decide whether that tmpfs is writable. The standard
//!   stream and terminal devices of `default_writes.rs` are always provided,
//!   so an `allow_write` naming one is accepted and reported as suppressed.
//! - A deny at a link name protects the target, not the link name. The same
//!   host directory bound at two guest paths is not de-aliased. Policy paths
//!   that do not exist in the guest compose nothing.
//! - Denying `/` hides every base mount. `/dev/shm` stays as bubblewrap
//!   provides it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};

use thiserror::Error;
use trogon_sandbox_contract::{
    FilesystemOptions, FsPathEntry, FsPolicy, FsPolicyError, FsPolicyList, GlobPattern, LiteralPath, LiteralPathError,
    MandatoryDenySearchDepth,
};
use trogon_sandbox_protocol::{
    FsPolicyEvaluator, GlobCursor, GlobMatcher, LiteralPathExt, search_depth_reaches_directory,
    search_depth_reaches_file,
};

use super::default_writes::is_default_write_device;
use super::mandatory;

use super::constants::{HOST_VIEW_PRIVATE_REGIONS, MAX_GIT_POINTER_BYTES, PRIVATE_REGIONS};
pub use super::constants::{
    MAX_BWRAP_ARGUMENTS, MAX_GLOB_VISITS, MAX_MANDATORY_VISITS, MAX_SYMLINK_HOPS, RESERVED_BWRAP_ARGUMENTS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostNodeId {
    pub dev: u64,
    pub ino: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Dir,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostNode {
    pub kind: NodeKind,
    pub id: HostNodeId,
}

#[derive(Debug, Error)]
pub enum HostTreeError {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
}

pub trait HostTree {
    /// Describes the entry itself, never following a final symlink.
    fn stat(&self, path: &Path) -> Result<Option<HostNode>, HostTreeError>;
    fn read_link(&self, path: &Path) -> Result<PathBuf, HostTreeError>;
    fn read_dir(&self, path: &Path) -> Result<Vec<OsString>, HostTreeError>;
    /// The first `max_bytes` of a regular file, never following a final
    /// symlink. `None` when the path is absent or not a regular file.
    fn read_file(&self, path: &Path, max_bytes: usize) -> Result<Option<Vec<u8>>, HostTreeError>;
    /// Whether the worker could create an entry inside the directory.
    fn can_create_in(&self, dir: &Path) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountAccess {
    ReadOnly,
    ReadWrite,
}

impl MountAccess {
    fn is_writable(self) -> bool {
        matches!(self, Self::ReadWrite)
    }

    fn from_writable(writable: bool) -> Self {
        if writable { Self::ReadWrite } else { Self::ReadOnly }
    }
}

#[derive(Debug, Clone)]
pub enum LayoutEntry {
    Bind {
        dest: LiteralPath,
        host: PathBuf,
        access: MountAccess,
    },
    Symlink {
        dest: LiteralPath,
        target: String,
    },
}

impl LayoutEntry {
    fn dest(&self) -> &LiteralPath {
        match self {
            Self::Bind { dest, .. } | Self::Symlink { dest, .. } => dest,
        }
    }
}

/// The mounts the sandbox gets before any policy is applied. Host paths must
/// already be canonical (no symlink in them).
#[derive(Debug, Clone, Default)]
pub struct GuestLayout {
    entries: Vec<LayoutEntry>,
    host_view: bool,
    working_dir: Option<LiteralPath>,
    masked_files: BTreeSet<LiteralPath>,
}

struct BindRef<'a> {
    dest: &'a LiteralPath,
    host: &'a Path,
    access: MountAccess,
}

impl GuestLayout {
    pub fn new() -> Self {
        Self::default()
    }

    /// A layout whose root is the host's, so `/tmp` is the host's directory
    /// like any other and only `/proc` and `/dev` stay private.
    pub fn host_view() -> Self {
        Self {
            host_view: true,
            ..Self::default()
        }
    }

    /// Where the command runs. The mandatory read-only paths are looked for
    /// from here; without it every outermost writable root
    /// stands in for it.
    pub fn with_working_dir(mut self, working_dir: LiteralPath) -> Self {
        self.working_dir = Some(working_dir);
        self
    }

    fn is_private(&self, path: &LiteralPath) -> bool {
        let regions: &[&str] = if self.host_view {
            &HOST_VIEW_PRIVATE_REGIONS
        } else {
            &PRIVATE_REGIONS
        };
        regions
            .iter()
            .any(|region| path.as_str() == *region || path.as_str().starts_with(&format!("{region}/")))
    }

    pub fn bind(&mut self, dest: LiteralPath, host: PathBuf, access: MountAccess) -> &mut Self {
        self.entries.push(LayoutEntry::Bind { dest, host, access });
        self
    }

    /// Marks a file bind as a credential mask: a read deny of the same file
    /// leaves the mask in place, since only the fake copy is ever readable
    /// there.
    pub fn mask_file(&mut self, dest: LiteralPath) -> &mut Self {
        self.masked_files.insert(dest);
        self
    }

    pub fn symlink(&mut self, dest: LiteralPath, target: String) -> &mut Self {
        self.entries.push(LayoutEntry::Symlink { dest, target });
        self
    }

    pub fn entries(&self) -> &[LayoutEntry] {
        &self.entries
    }

    fn binds(&self) -> impl Iterator<Item = BindRef<'_>> {
        self.entries.iter().filter_map(|entry| match entry {
            LayoutEntry::Bind { dest, host, access } => Some(BindRef {
                dest,
                host,
                access: *access,
            }),
            LayoutEntry::Symlink { .. } => None,
        })
    }

    fn covering_bind(&self, path: &LiteralPath) -> Option<BindRef<'_>> {
        self.binds()
            .filter(|bind| path.is_within(bind.dest))
            .max_by_key(|bind| bind.dest.components().count())
    }

    fn symlink_at(&self, path: &LiteralPath) -> Option<(&LiteralPath, &str)> {
        self.entries.iter().find_map(|entry| match entry {
            LayoutEntry::Symlink { dest, target } if dest == path => Some((dest, target.as_str())),
            _ => None,
        })
    }

    fn has_descendant(&self, path: &LiteralPath) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.dest() != path && entry.dest().is_within(path))
    }

    fn child_names(&self, path: &LiteralPath) -> Vec<String> {
        let depth = path.components().count();
        self.entries
            .iter()
            .filter(|entry| entry.dest() != path && entry.dest().is_within(path))
            .filter_map(|entry| entry.dest().components().nth(depth).map(str::to_string))
            .collect()
    }

    fn host_path(&self, path: &LiteralPath) -> Option<PathBuf> {
        let bind = self.covering_bind(path)?;
        let skip = bind.dest.components().count();
        let mut host = bind.host.to_path_buf();
        host.extend(path.components().skip(skip));
        Some(host)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSource {
    pub path: PathBuf,
    pub id: HostNodeId,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountOp {
    Bind {
        source: HostSource,
        dest: LiteralPath,
        access: MountAccess,
    },
    EmptyDir {
        dest: LiteralPath,
    },
    FileStub {
        dest: LiteralPath,
    },
    Symlink {
        target: String,
        dest: LiteralPath,
    },
    RemountReadOnly {
        dest: LiteralPath,
    },
}

impl MountOp {
    pub fn dest(&self) -> &LiteralPath {
        match self {
            Self::Bind { dest, .. }
            | Self::EmptyDir { dest }
            | Self::FileStub { dest }
            | Self::Symlink { dest, .. }
            | Self::RemountReadOnly { dest } => dest,
        }
    }

    pub fn arguments(&self) -> usize {
        match self {
            Self::Bind { .. } | Self::FileStub { .. } | Self::Symlink { .. } => 3,
            Self::EmptyDir { .. } | Self::RemountReadOnly { .. } => 2,
        }
    }
}

impl fmt::Display for MountOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind { dest, access, .. } => {
                let mode = if access.is_writable() { "rw" } else { "ro" };
                write!(f, "bind {mode} {dest}")
            }
            Self::EmptyDir { dest } => write!(f, "tmpfs {dest}"),
            Self::FileStub { dest } => write!(f, "stub {dest}"),
            Self::Symlink { target, dest } => write!(f, "symlink {target} {dest}"),
            Self::RemountReadOnly { dest } => write!(f, "remount-ro {dest}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressedReason {
    ViaSymlink,
    HiddenFromReads,
    ReadOnlyMount,
    ProvidedBySandbox,
}

impl fmt::Display for SuppressedReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ViaSymlink => "reached through a symlink",
            Self::HiddenFromReads => "hidden by deny_read",
            Self::ReadOnlyMount => "its mount is read-only",
            Self::ProvidedBySandbox => "the sandbox always provides it",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuppressedEntry {
    pub list: FsPolicyList,
    pub path: LiteralPath,
    pub reason: SuppressedReason,
}

#[derive(Debug, Clone)]
pub struct MountPlan {
    pub ops: Vec<MountOp>,
    pub tmp_writable: bool,
    /// Host paths bubblewrap creates as mount points and leaves behind.
    pub placeholders: Vec<PathBuf>,
    /// Only ever holds deny-read paths that do not exist and could not be
    /// guarded because the worker cannot create the mount point. Anything the
    /// sandbox creates there is its own data. An unguardable deny-write path
    /// is refused with `FsPlanError::UnguardableDenyWrite` instead.
    pub unguarded: Vec<LiteralPath>,
    pub suppressed: Vec<SuppressedEntry>,
    /// Every path protected by the mandatory list, including the ones that do
    /// not exist yet and are guarded.
    pub mandatory: Vec<LiteralPath>,
}

impl MountPlan {
    pub fn bind_sources(&self) -> impl Iterator<Item = &HostSource> {
        self.ops.iter().filter_map(|op| match op {
            MountOp::Bind { source, .. } => Some(source),
            _ => None,
        })
    }

    /// Whether the plan binds the root itself writable, as turning the
    /// filesystem rules off does; remounting it read-only would undo that.
    pub fn root_writable(&self) -> bool {
        self.ops
            .iter()
            .any(|op| matches!(op, MountOp::Bind { dest, access, .. } if dest.is_root() && access.is_writable()))
    }

    /// Bubblewrap arguments for the plan. `fds` holds one descriptor per
    /// [`MountOp::Bind`], in plan order, opened from [`MountPlan::bind_sources`].
    pub fn render(&self, fds: &[RawFd]) -> Result<Vec<String>, FsPlanError> {
        let mut fds = fds.iter();
        let mut args = Vec::new();
        for op in &self.ops {
            match op {
                MountOp::Bind { dest, access, .. } => {
                    let flag = if access.is_writable() {
                        "--bind-fd"
                    } else {
                        "--ro-bind-fd"
                    };
                    let fd = fds
                        .next()
                        .ok_or_else(|| FsPlanError::MissingDescriptor { dest: dest.to_string() })?;
                    args.extend([flag.to_string(), fd.to_string(), dest.to_string()]);
                }
                MountOp::EmptyDir { dest } => args.extend(["--tmpfs".to_string(), dest.to_string()]),
                MountOp::FileStub { dest } => {
                    args.extend(["--ro-bind".to_string(), "/dev/null".to_string(), dest.to_string()])
                }
                MountOp::Symlink { target, dest } => {
                    args.extend(["--symlink".to_string(), target.clone(), dest.to_string()])
                }
                MountOp::RemountReadOnly { dest } => args.extend(["--remount-ro".to_string(), dest.to_string()]),
            }
        }
        Ok(args)
    }
}

#[derive(Debug, Error)]
pub enum FsPlanError {
    #[error(transparent)]
    Policy(#[from] FsPolicyError),
    #[error(transparent)]
    Path(#[from] LiteralPathError),
    #[error(transparent)]
    Host(#[from] HostTreeError),
    #[error("more than {MAX_SYMLINK_HOPS} symlinks while resolving {path}")]
    SymlinkLoop { path: LiteralPath },
    #[error(
        "{path} is inside a region the sandbox provides itself (/proc, /dev, /tmp); only \
         allow_write and deny_write of exactly /tmp are accepted there"
    )]
    PrivateRegion { path: String },
    #[error(
        "deny_write pattern {pattern} overlaps a writable grant, and a pattern cannot protect \
         files created after the plan is built. Name the paths as literals"
    )]
    UnenforceableWriteGlob { pattern: String },
    #[error(
        "deny_write path {0} does not exist and the worker cannot create a mount point to guard \
         it, so the sandbox could create it"
    )]
    UnguardableDenyWrite(LiteralPath),
    #[error("pattern {pattern} visits more than {MAX_GLOB_VISITS} entries")]
    GlobTooLarge { pattern: String },
    #[error(
        "scanning {root} for mandatory read-only paths visits more than {MAX_MANDATORY_VISITS} \
         entries"
    )]
    MandatoryScanTooLarge { root: LiteralPath },
    #[error(
        "the mounts have {total} bwrap arguments ({mandatory} for the mandatory read-only \
         paths, {configured} for the configuration) and bwrap accepts at most \
         {MAX_BWRAP_ARGUMENTS}, about {} mounts; lower mandatoryDenySearchDepth, run from a \
         smaller directory or name fewer paths",
        MAX_BWRAP_ARGUMENTS / 3
    )]
    TooManyArguments {
        total: usize,
        mandatory: usize,
        configured: usize,
    },
    #[error("{} changed between planning and mounting", path.display())]
    SourceChanged { path: PathBuf },
    #[error("base mount source {} does not exist", host.display())]
    MissingBase { host: PathBuf },
    #[error("opening {}: {source}", path.display())]
    OpenSource { path: PathBuf, source: io::Error },
    #[error("no descriptor was opened for the bind at {dest}")]
    MissingDescriptor { dest: String },
}

impl FsPlanError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Policy(_) => "policy",
            Self::Path(_) => "path",
            Self::Host(_) => "host",
            Self::SymlinkLoop { .. } => "symlink-loop",
            Self::PrivateRegion { .. } => "private-region",
            Self::UnenforceableWriteGlob { .. } => "unenforceable-write-glob",
            Self::UnguardableDenyWrite(_) => "unguardable-deny-write",
            Self::GlobTooLarge { .. } => "glob-too-large",
            Self::MandatoryScanTooLarge { .. } => "mandatory-scan-too-large",
            Self::TooManyArguments { .. } => "too-many-arguments",
            Self::SourceChanged { .. } => "source-changed",
            Self::MissingBase { .. } => "missing-base",
            Self::OpenSource { .. } => "open-source",
            Self::MissingDescriptor { .. } => "missing-descriptor",
        }
    }
}

pub fn plan_mounts<T: HostTree>(policy: &FsPolicy, layout: &GuestLayout, tree: &T) -> Result<MountPlan, FsPlanError> {
    plan_mounts_with(policy, layout, tree, &FilesystemOptions::default())
}

pub fn plan_mounts_with<T: HostTree>(
    policy: &FsPolicy,
    layout: &GuestLayout,
    tree: &T,
    options: &FilesystemOptions,
) -> Result<MountPlan, FsPlanError> {
    Planner { layout, tree, options }.plan(policy)
}

/// The root of the guest filesystem.
pub(super) fn root_path() -> Result<LiteralPath, LiteralPathError> {
    LiteralPath::new("/")
}

fn child(parent: &LiteralPath, name: &str) -> Result<LiteralPath, FsPlanError> {
    let raw = if parent.is_root() {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    };
    Ok(LiteralPath::new(&raw)?)
}

fn depth(path: &LiteralPath) -> usize {
    path.components().count()
}

#[derive(Debug, Clone)]
enum Source {
    Synthetic,
    Host { path: PathBuf, node: HostNode },
}

impl Source {
    fn kind(&self) -> NodeKind {
        match self {
            Self::Synthetic => NodeKind::Dir,
            Self::Host { node, .. } => node.kind,
        }
    }

    fn host_source(&self) -> Option<HostSource> {
        match self {
            Self::Synthetic => None,
            Self::Host { path, node } => Some(HostSource {
                path: path.clone(),
                id: node.id,
                kind: node.kind,
            }),
        }
    }
}

#[derive(Debug, Clone)]
struct Found {
    canon: LiteralPath,
    source: Source,
    via_symlink: bool,
}

#[derive(Clone)]
enum Outcome {
    Found(Found),
    Absent { guard: LiteralPath },
    Held(Found),
    Unreachable,
    Private,
}

#[derive(Default)]
struct GlobExpansion {
    found: Vec<Found>,
    unlisted: Vec<Found>,
}

struct GlobWalk<'a> {
    visited: usize,
    seen: HashSet<(LiteralPath, GlobCursor, bool)>,
    carve_outs: &'a [LiteralPath],
    directory_form: bool,
}

struct GlobDir {
    lexical: LiteralPath,
    canon: LiteralPath,
    via_symlink: bool,
    chain: Vec<LiteralPath>,
    cursor: GlobCursor,
    masked_root: Option<LiteralPath>,
}

#[derive(Default)]
struct Targets {
    found: Vec<Found>,
    absent: Vec<LiteralPath>,
    held: Vec<Found>,
    unreadable: Vec<Found>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    Stub,
    Mount { writable: bool },
}

struct Candidate {
    source: Option<HostSource>,
    kind: NodeKind,
    is_base: bool,
}

struct ReadDeny {
    path: LiteralPath,
    lifted_by: Vec<LiteralPath>,
}

impl ReadDeny {
    fn hides(&self, path: &LiteralPath) -> bool {
        path.is_within(&self.path) && !self.lifted_by.iter().any(|allow| path.is_within(allow))
    }
}

struct State {
    deny_read: Vec<ReadDeny>,
    grants: Vec<LiteralPath>,
    deny_write: Vec<LiteralPath>,
}

impl State {
    fn hidden(&self, path: &LiteralPath) -> bool {
        self.deny_read.iter().any(|deny| deny.hides(path))
    }

    fn writable(&self, layout: &GuestLayout, path: &LiteralPath) -> bool {
        self.grants.iter().any(|grant| path.is_within(grant))
            && !self.deny_write.iter().any(|deny| path.is_within(deny))
            && !self.hidden(path)
            && layout.covering_bind(path).is_some_and(|bind| bind.access.is_writable())
    }
}

/// Where an op sits in the emitted order. Under a `/` write root a pin goes in
/// after the mounts that make its directory visible and a cover after the
/// pins, so what the cover holds is emitted after it: the pins keep the
/// kernel from renaming the directory but nothing reaches them by name.
#[derive(Clone, Copy)]
enum Step {
    Plain,
    Pin,
    Cover,
    Covered,
}

#[derive(Default)]
struct Emitter {
    ops: BTreeMap<(u8, usize, LiteralPath), MountOp>,
    regions: HashMap<LiteralPath, Region>,
}

impl Emitter {
    fn enclosing(&self, path: &LiteralPath) -> Region {
        path.ancestors()
            .skip(1)
            .find_map(|ancestor| self.regions.get(&ancestor).copied())
            .unwrap_or(Region::Mount { writable: false })
    }

    fn insert(&mut self, dest: &LiteralPath, op: MountOp, region: Region) {
        self.insert_in(Step::Plain, dest, op, region);
    }

    fn insert_in(&mut self, step: Step, dest: &LiteralPath, op: MountOp, region: Region) {
        self.ops.insert((step as u8, 2 * depth(dest) + 1, dest.clone()), op);
        self.regions.insert(dest.clone(), region);
    }

    /// Emits `op` just before the mounts at `anchor_depth`, so the mount at
    /// that depth that holds `dest` is bound over it.
    fn insert_beneath(&mut self, anchor_depth: usize, dest: &LiteralPath, op: MountOp, region: Region) {
        self.ops.insert((Step::Plain as u8, 2 * anchor_depth, dest.clone()), op);
        self.regions.insert(dest.clone(), region);
    }

    fn contains(&self, dest: &LiteralPath) -> bool {
        self.regions.contains_key(dest)
    }
}

struct Planner<'a, T> {
    layout: &'a GuestLayout,
    tree: &'a T,
    options: &'a FilesystemOptions,
}

impl<T: HostTree> Planner<'_, T> {
    fn source_of(&self, path: &LiteralPath) -> Result<Option<Source>, FsPlanError> {
        if path.is_root() {
            return Ok(Some(Source::Synthetic));
        }
        if let Some(host) = self.layout.host_path(path) {
            return Ok(self.tree.stat(&host)?.map(|node| Source::Host { path: host, node }));
        }
        if self.layout.has_descendant(path) {
            return Ok(Some(Source::Synthetic));
        }
        Ok(None)
    }

    fn splice(
        &self,
        pending: &mut VecDeque<String>,
        cur: &mut LiteralPath,
        src: &mut Source,
        target: &str,
    ) -> Result<(), LiteralPathError> {
        if target.starts_with('/') {
            *cur = root_path()?;
            *src = Source::Synthetic;
        }
        for component in target.split('/').filter(|c| !c.is_empty()).rev() {
            pending.push_front(component.to_string());
        }
        Ok(())
    }

    fn resolve(&self, path: &LiteralPath) -> Result<Outcome, FsPlanError> {
        self.resolve_components(path, path.components().map(str::to_string).collect())
    }

    fn resolve_components(&self, path: &LiteralPath, mut pending: VecDeque<String>) -> Result<Outcome, FsPlanError> {
        let mut cur = root_path()?;
        let mut src = Source::Synthetic;
        let mut hops = 0usize;
        let mut via_symlink = false;

        while let Some(name) = pending.pop_front() {
            if name == "." {
                continue;
            }
            if name == ".." {
                cur = match cur.parent() {
                    Some(parent) => parent,
                    None => root_path()?,
                };
                src = self.source_of(&cur)?.unwrap_or(Source::Synthetic);
                continue;
            }
            if src.kind() != NodeKind::Dir {
                return Ok(Outcome::Held(Found {
                    canon: cur,
                    source: src,
                    via_symlink,
                }));
            }
            let candidate = child(&cur, &name)?;
            if self.layout.is_private(&candidate) {
                return Ok(Outcome::Private);
            }
            if let Some((_, target)) = self.layout.symlink_at(&candidate) {
                hops += 1;
                if hops > MAX_SYMLINK_HOPS {
                    return Err(FsPlanError::SymlinkLoop { path: path.clone() });
                }
                via_symlink = true;
                self.splice(&mut pending, &mut cur, &mut src, target)?;
                continue;
            }
            match self.source_of(&candidate)? {
                None => {
                    return Ok(if self.layout.covering_bind(&candidate).is_some() {
                        Outcome::Absent { guard: candidate }
                    } else {
                        Outcome::Unreachable
                    });
                }
                Some(Source::Host { path: host, node }) if node.kind == NodeKind::Symlink => {
                    hops += 1;
                    if hops > MAX_SYMLINK_HOPS {
                        return Err(FsPlanError::SymlinkLoop { path: path.clone() });
                    }
                    via_symlink = true;
                    let target = self.tree.read_link(&host)?;
                    let Ok(target) = target.into_os_string().into_string() else {
                        tracing::warn!(
                            %candidate,
                            "link to a name that is no text and cannot be told where it leads (EILSEQ): mounting it on its own spelling"
                        );
                        cur = candidate;
                        src = Source::Host { path: host, node };
                        continue;
                    };
                    self.splice(&mut pending, &mut cur, &mut src, &target)?;
                }
                Some(found) => {
                    cur = candidate;
                    src = found;
                }
            }
        }
        Ok(Outcome::Found(Found {
            canon: cur,
            source: src,
            via_symlink,
        }))
    }

    fn list_dir(&self, dir: &LiteralPath) -> Result<BTreeSet<String>, FsPlanError> {
        let mut names: BTreeSet<String> = self.layout.child_names(dir).into_iter().collect();
        if let Some(host) = self.layout.host_path(dir)
            && self.tree.stat(&host)?.is_some_and(|node| node.kind == NodeKind::Dir)
        {
            for name in self.tree.read_dir(&host)? {
                match name.into_string() {
                    Ok(name) => {
                        names.insert(name);
                    }
                    Err(name) => {
                        tracing::warn!(?name, %dir, "name is no text and cannot be matched by a pattern (EILSEQ)")
                    }
                }
            }
        }
        Ok(names)
    }

    fn expand_glob(
        &self,
        pattern: &GlobPattern,
        carve_outs: &[LiteralPath],
        directory_form: bool,
    ) -> Result<GlobExpansion, FsPlanError> {
        let mut expansion = GlobExpansion::default();
        let matcher = GlobMatcher::new(pattern);
        let base = matcher.static_base()?;
        if base.is_root() {
            tracing::warn!(%pattern, "pattern has no directory before its first wildcard and is not expanded");
            return Ok(expansion);
        }
        let root = match self.resolve(&base) {
            Ok(Outcome::Found(root)) => root,
            Ok(_) => return Ok(expansion),
            Err(FsPlanError::Host(HostTreeError::Io { source, .. })) if source.kind() != io::ErrorKind::NotFound => {
                expansion.unlisted.extend(self.reachable_ancestor(&base)?);
                return Ok(expansion);
            }
            Err(other) => return Err(other),
        };
        if root.source.kind() != NodeKind::Dir {
            return Ok(expansion);
        }
        if matcher.matches(&base) {
            expansion.found.push(root.clone());
        }
        let mut walk = GlobWalk {
            visited: 0,
            seen: HashSet::new(),
            carve_outs,
            directory_form,
        };
        let first = GlobDir {
            cursor: matcher.cursor(&base),
            lexical: base,
            canon: root.canon.clone(),
            via_symlink: root.via_symlink,
            chain: vec![root.canon.clone()],
            masked_root: None,
        };
        walk.seen
            .insert((first.canon.clone(), first.cursor.clone(), first.via_symlink));
        let mut stack = vec![first];
        while let Some(dir) = stack.pop() {
            let mark = (expansion.found.len(), stack.len());
            match self.glob_dir(pattern, &matcher, &dir, &mut walk, &mut expansion, &mut stack) {
                Err(FsPlanError::Host(HostTreeError::Io { source, .. }))
                    if source.kind() != io::ErrorKind::NotFound =>
                {
                    expansion.found.truncate(mark.0);
                    stack.truncate(mark.1);
                    if let Outcome::Found(holder) = self.resolve(&dir.canon)? {
                        expansion.unlisted.push(holder);
                    }
                }
                other => other?,
            }
        }
        Ok(expansion)
    }

    /// The nearest directory above `path` that can still be looked at, which
    /// is the one hiding everything below it.
    fn reachable_ancestor(&self, path: &LiteralPath) -> Result<Option<Found>, FsPlanError> {
        for ancestor in path.ancestors().skip(1).filter(|a| !a.is_root()) {
            match self.resolve(&ancestor) {
                Ok(Outcome::Found(found)) => return Ok(Some(found)),
                Ok(_) => {}
                Err(FsPlanError::Host(HostTreeError::Io { source, .. }))
                    if source.kind() != io::ErrorKind::NotFound => {}
                Err(other) => return Err(other),
            }
        }
        Ok(None)
    }

    fn glob_dir(
        &self,
        pattern: &GlobPattern,
        matcher: &GlobMatcher,
        dir: &GlobDir,
        walk: &mut GlobWalk<'_>,
        expansion: &mut GlobExpansion,
        stack: &mut Vec<GlobDir>,
    ) -> Result<(), FsPlanError> {
        for name in self.list_dir(&dir.canon)? {
            let canon_child = child(&dir.canon, &name)?;
            if self.layout.is_private(&canon_child) {
                continue;
            }
            walk.visited += 1;
            if walk.visited > MAX_GLOB_VISITS {
                return Err(FsPlanError::GlobTooLarge {
                    pattern: pattern.to_string(),
                });
            }
            let cursor = dir.cursor.child(matcher, &name);
            if cursor.is_dead() {
                continue;
            }
            let lexical_child = child(&dir.lexical, &name)?;
            let matched = matcher.matches(&lexical_child);
            let directory_form = walk.directory_form && matcher.matches_directory_form(&lexical_child);
            let node = self.source_of(&canon_child)?;
            let is_link = node.as_ref().is_some_and(|source| source.kind() == NodeKind::Symlink);
            let followed = if matched || directory_form || is_link {
                match self.resolve(&canon_child) {
                    Ok(outcome) => Some(outcome),
                    Err(FsPlanError::Host(HostTreeError::Io { source, .. }))
                        if source.kind() != io::ErrorKind::NotFound =>
                    {
                        if matched || directory_form {
                            tracing::warn!(
                                %pattern,
                                %canon_child,
                                "child leads somewhere that cannot be inspected; denying what holds it"
                            );
                            if let Outcome::Found(holder) = self.stand_in(&canon_child)? {
                                expansion.found.push(holder);
                            }
                        }
                        continue;
                    }
                    Err(other) => return Err(other),
                }
            } else {
                None
            };
            if directory_form
                && let Some(Outcome::Found(mut found)) = followed.clone()
                && found.source.kind() == NodeKind::Dir
                && !found.canon.is_root()
                && (found.via_symlink || !self.list_dir(&found.canon)?.is_empty())
            {
                found.via_symlink |= dir.via_symlink;
                expansion.found.push(found);
            }
            if matched {
                if let Some(Outcome::Found(mut found)) = followed.clone() {
                    if found.canon.is_root() {
                        if let Outcome::Found(holder) = self.resolve(&dir.canon)? {
                            expansion.found.push(holder);
                        }
                        continue;
                    }
                    found.via_symlink |= dir.via_symlink;
                    expansion.found.push(found);
                }
                if walk.carve_outs.is_empty() {
                    continue;
                }
            }
            let descend = match node {
                Some(source) if source.kind() == NodeKind::Dir => Some((canon_child.clone(), dir.via_symlink)),
                Some(source) if source.kind() == NodeKind::Symlink => match followed {
                    Some(Outcome::Found(target))
                        if target.source.kind() == NodeKind::Dir
                            && !(target.canon.is_root()
                                || dir.canon.is_within(&target.canon)
                                || dir.chain.contains(&target.canon)) =>
                    {
                        Some((target.canon, true))
                    }
                    _ => None,
                },
                _ => None,
            };
            let Some((canon, via_symlink)) = descend else {
                continue;
            };
            if !cursor.reaches_beneath(matcher) {
                continue;
            }
            if matched {
                let carved = |root: &LiteralPath| {
                    walk.carve_outs.iter().any(|allowed| {
                        allowed.is_within(root) && (allowed.is_within(&canon) || canon.is_within(allowed))
                    })
                };
                let toward = walk.carve_outs.iter().any(|allowed| allowed.is_within(&canon));
                if !(toward || dir.masked_root.as_ref().is_some_and(carved)) {
                    continue;
                }
            }
            if !walk.seen.insert((canon.clone(), cursor.clone(), via_symlink)) {
                continue;
            }
            let mut chain = dir.chain.clone();
            chain.push(canon.clone());
            let masked_root = dir.masked_root.clone().or_else(|| matched.then(|| canon.clone()));
            stack.push(GlobDir {
                lexical: lexical_child,
                canon,
                via_symlink,
                chain,
                cursor,
                masked_root,
            });
        }
        Ok(())
    }

    /// Where a read deny lands when the path itself cannot be inspected: the
    /// deepest directory above it that can, which hides more than was asked
    /// and never less. Nothing is hidden when no directory
    /// above it can be inspected either, or when one of them is absent.
    fn stand_in(&self, path: &LiteralPath) -> Result<Outcome, FsPlanError> {
        for ancestor in path.ancestors().skip(1) {
            if ancestor.is_root() {
                break;
            }
            match self.resolve(&ancestor) {
                Ok(Outcome::Found(found)) => return Ok(Outcome::Found(found)),
                Ok(_) => break,
                Err(FsPlanError::Host(_)) => continue,
                Err(other) => return Err(other),
            }
        }
        Ok(Outcome::Unreachable)
    }

    fn gather(
        &self,
        list: FsPolicyList,
        entries: &[FsPathEntry],
        skip_tmp: bool,
        carve_outs: &[LiteralPath],
        out: &mut Targets,
        suppressed: &mut Vec<SuppressedEntry>,
    ) -> Result<(), FsPlanError> {
        let is_allow = matches!(
            list,
            FsPolicyList::AllowReadWithinDeny | FsPolicyList::OwnAllowWithinDeny | FsPolicyList::AllowWrite
        );
        let accept = |found: Found, out: &mut Targets, suppressed: &mut Vec<SuppressedEntry>| {
            if is_allow && found.via_symlink {
                suppressed.push(SuppressedEntry {
                    list,
                    path: found.canon,
                    reason: SuppressedReason::ViaSymlink,
                });
            } else {
                out.found.push(found);
            }
        };
        for entry in entries {
            match entry {
                FsPathEntry::Literal { path, .. } => {
                    if skip_tmp && path.as_str() == "/tmp" {
                        continue;
                    }
                    if list == FsPolicyList::AllowWrite && is_default_write_device(path.as_str()) {
                        suppressed.push(SuppressedEntry {
                            list,
                            path: path.clone(),
                            reason: SuppressedReason::ProvidedBySandbox,
                        });
                        continue;
                    }
                    if self.layout.is_private(path) {
                        return Err(FsPlanError::PrivateRegion { path: path.to_string() });
                    }
                    let outcome = match self.resolve(path) {
                        Err(FsPlanError::Host(_))
                            if matches!(list, FsPolicyList::DenyRead | FsPolicyList::CredentialDenyRead) =>
                        {
                            self.stand_in(path)?
                        }
                        other => other?,
                    };
                    match outcome {
                        Outcome::Found(found) => accept(found, out, suppressed),
                        Outcome::Absent { guard } if !is_allow => out.absent.push(guard),
                        Outcome::Held(found) if !is_allow => out.held.push(found),
                        Outcome::Private => {
                            return Err(FsPlanError::PrivateRegion { path: path.to_string() });
                        }
                        Outcome::Absent { .. } | Outcome::Held(_) | Outcome::Unreachable => {}
                    }
                }
                FsPathEntry::Glob { pattern } => {
                    if self.layout.is_private(&GlobMatcher::new(pattern).static_base()?) {
                        return Err(FsPlanError::PrivateRegion {
                            path: pattern.to_string(),
                        });
                    }
                    let reads_allow = matches!(
                        list,
                        FsPolicyList::AllowReadWithinDeny | FsPolicyList::OwnAllowWithinDeny
                    );
                    let expansion = self.expand_glob(pattern, carve_outs, !reads_allow)?;
                    for found in expansion.found {
                        accept(found, out, suppressed);
                    }
                    if !is_allow && list != FsPolicyList::DenyWrite {
                        out.unreadable.extend(expansion.unlisted);
                    }
                }
            }
        }
        Ok(())
    }

    fn plan(&self, policy: &FsPolicy) -> Result<MountPlan, FsPlanError> {
        let tmp = LiteralPath::new("/tmp")?;
        let names_tmp = |entries: &[FsPathEntry]| {
            entries
                .iter()
                .any(|entry| matches!(entry, FsPathEntry::Literal { path, .. } if *path == tmp))
        };
        let tmp_writable = !self.layout.host_view && names_tmp(policy.allow_write()) && !names_tmp(policy.deny_write());
        let skip_tmp = !self.layout.host_view;

        let mut suppressed = Vec::new();
        let mut deny_read = Targets::default();
        let mut allow_read = Targets::default();
        let mut allow_write = Targets::default();
        let mut deny_write = Targets::default();
        let mut allow_read_canons: Vec<Vec<LiteralPath>> = Vec::new();
        let mut restores: Vec<(LiteralPath, Found)> = Vec::new();
        for entry in policy.allow_read_within_deny() {
            if let FsPathEntry::Literal { path, .. } = entry
                && !self.layout.is_private(path)
                && let Outcome::Found(found) = self.resolve(path)?
                && found.via_symlink
            {
                restores.push((path.clone(), found));
                allow_read_canons.push(Vec::new());
                continue;
            }
            let mut entry_targets = Targets::default();
            self.gather(
                FsPolicyList::AllowReadWithinDeny,
                std::slice::from_ref(entry),
                false,
                &[],
                &mut entry_targets,
                &mut suppressed,
            )?;
            allow_read_canons.push(entry_targets.found.iter().map(|f| f.canon.clone()).collect());
            allow_read.found.extend(entry_targets.found);
        }
        let mut own_allow_canons: Vec<Vec<LiteralPath>> = Vec::new();
        for entry in policy.own_allow_within_deny() {
            let mut entry_targets = Targets::default();
            self.gather(
                FsPolicyList::OwnAllowWithinDeny,
                std::slice::from_ref(entry),
                false,
                &[],
                &mut entry_targets,
                &mut suppressed,
            )?;
            own_allow_canons.push(entry_targets.found.iter().map(|f| f.canon.clone()).collect());
            allow_read.found.extend(entry_targets.found);
        }
        let carve_outs: Vec<LiteralPath> = allow_read_canons
            .iter()
            .chain(&own_allow_canons)
            .flatten()
            .cloned()
            .collect();
        let evaluator = FsPolicyEvaluator::new(policy);
        let mut read_denies: Vec<ReadDeny> = Vec::new();
        let read_deny_lists = [
            (FsPolicyList::DenyRead, policy.deny_read()),
            (FsPolicyList::CredentialDenyRead, policy.credential_deny_read()),
        ];
        for (list, entry) in read_deny_lists
            .into_iter()
            .flat_map(|(list, entries)| entries.iter().map(move |entry| (list, entry)))
        {
            let mut entry_targets = Targets::default();
            self.gather(
                list,
                std::slice::from_ref(entry),
                false,
                &carve_outs,
                &mut entry_targets,
                &mut suppressed,
            )?;
            let credential = list == FsPolicyList::CredentialDenyRead;
            let lifted_by: Vec<LiteralPath> = if credential {
                evaluator
                    .credential_read_exceptions(entry)
                    .into_iter()
                    .flat_map(|index| own_allow_canons[index].iter().cloned())
                    .collect()
            } else {
                evaluator
                    .read_exceptions(entry)
                    .into_iter()
                    .flat_map(|index| allow_read_canons[index].iter().cloned())
                    .collect()
            };
            let is_glob = matches!(entry, FsPathEntry::Glob { .. });
            read_denies.extend(entry_targets.found.iter().map(|found| {
                let mut lifted_by = lifted_by.clone();
                if is_glob {
                    lifted_by.retain(|allowed| found.canon == *allowed || !found.canon.is_within(allowed));
                }
                {
                    let pool = if credential {
                        &own_allow_canons
                    } else {
                        &allow_read_canons
                    };
                    lifted_by.extend(
                        pool.iter()
                            .flatten()
                            .filter(|allowed| allowed.is_within(&found.canon))
                            .cloned(),
                    );
                }
                ReadDeny {
                    path: found.canon.clone(),
                    lifted_by,
                }
            }));
            read_denies.extend(entry_targets.unreadable.iter().map(|found| ReadDeny {
                path: found.canon.clone(),
                lifted_by: Vec::new(),
            }));
            deny_read.found.extend(entry_targets.found);
            deny_read.found.extend(entry_targets.unreadable);
            deny_read.absent.extend(entry_targets.absent);
            deny_read.held.extend(entry_targets.held);
        }
        self.gather(
            FsPolicyList::AllowWrite,
            policy.allow_write(),
            skip_tmp,
            &[],
            &mut allow_write,
            &mut suppressed,
        )?;
        let deny_write_literals: Vec<FsPathEntry> = policy
            .deny_write()
            .iter()
            .filter(|entry| !matches!(entry, FsPathEntry::Glob { .. }))
            .cloned()
            .collect();
        self.gather(
            FsPolicyList::DenyWrite,
            &deny_write_literals,
            skip_tmp,
            &[],
            &mut deny_write,
            &mut suppressed,
        )?;

        let mut state = State {
            deny_read: read_denies,
            grants: Vec::new(),
            deny_write: deny_write
                .found
                .iter()
                .chain(&deny_write.held)
                .chain(&deny_read.held)
                .map(|f| f.canon.clone())
                .collect(),
        };

        let mut grants = Vec::new();
        for found in allow_write.found {
            let reason = if state.hidden(&found.canon) {
                Some(SuppressedReason::HiddenFromReads)
            } else if !self
                .layout
                .covering_bind(&found.canon)
                .is_some_and(|bind| bind.access.is_writable())
            {
                Some(SuppressedReason::ReadOnlyMount)
            } else {
                None
            };
            match reason {
                Some(reason) => suppressed.push(SuppressedEntry {
                    list: FsPolicyList::AllowWrite,
                    path: found.canon,
                    reason,
                }),
                None => {
                    state.grants.push(found.canon.clone());
                    grants.push(found);
                }
            }
        }

        let direct: BTreeSet<&LiteralPath> = deny_write.found.iter().map(|f| &f.canon).collect();
        let alone: Vec<LiteralPath> = deny_write
            .held
            .iter()
            .chain(&deny_read.held)
            .map(|held| &held.canon)
            .filter(|file| {
                !direct.contains(file)
                    && state.grants.iter().any(|grant| grant == *file)
                    && !state.grants.iter().any(|grant| *file != grant && file.is_within(grant))
            })
            .cloned()
            .collect();
        state.deny_write.retain(|path| !alone.contains(path));

        let mandatory = if self.options.disabled.is_disabled() {
            Targets::default()
        } else {
            self.mandatory_targets(&grants, &state)?
        };
        let mut mandatory_paths: BTreeSet<LiteralPath> = mandatory
            .found
            .iter()
            .map(|found| found.canon.clone())
            .chain(mandatory.absent.iter().cloned())
            .collect();
        mandatory_paths.retain(|path| !path.is_root());
        let all_allowed: Vec<LiteralPath> = allow_read_canons.into_iter().flatten().collect();
        state
            .deny_read
            .extend(mandatory.unreadable.iter().map(|found| ReadDeny {
                path: found.canon.clone(),
                lifted_by: all_allowed.clone(),
            }));
        deny_read.found.extend(mandatory.unreadable);
        state.deny_write.extend(
            mandatory
                .found
                .iter()
                .chain(&mandatory.held)
                .map(|found| found.canon.clone()),
        );
        deny_write.found.extend(mandatory.found);
        deny_write.held.extend(mandatory.held);
        deny_write.absent.extend(mandatory.absent);

        self.refuse_unenforceable_write_globs(policy, &state)?;

        let mut candidates: BTreeMap<(usize, LiteralPath), Candidate> = BTreeMap::new();
        let mut add = |path: &LiteralPath, source: &Source, is_base: bool| {
            let entry = candidates
                .entry((depth(path), path.clone()))
                .or_insert_with(|| Candidate {
                    source: source.host_source(),
                    kind: source.kind(),
                    is_base,
                });
            if is_base && entry.is_base {
                entry.source = source.host_source();
                entry.kind = source.kind();
            }
            entry.is_base |= is_base;
        };

        for bind in self.layout.binds() {
            let host = bind.host.to_path_buf();
            let node = self
                .tree
                .stat(&host)?
                .ok_or_else(|| FsPlanError::MissingBase { host: host.clone() })?;
            add(bind.dest, &Source::Host { path: host, node }, true);
        }
        let protected = deny_read
            .found
            .iter()
            .chain(&allow_read.found)
            .chain(&grants)
            .chain(&deny_write.found)
            .chain(&deny_write.held)
            .chain(&deny_read.held);
        for found in protected {
            if !found.canon.is_root() {
                add(&found.canon, &found.source, false);
            }
        }

        let mut emitter = Emitter::default();
        for entry in self.layout.entries() {
            if let LayoutEntry::Symlink { dest, target } = entry {
                emitter.insert(
                    dest,
                    MountOp::Symlink {
                        target: target.clone(),
                        dest: dest.clone(),
                    },
                    Region::Mount { writable: false },
                );
            }
        }

        for ((_, dest), candidate) in &candidates {
            let enclosing = emitter.enclosing(dest);
            let masked = candidate.kind == NodeKind::File && self.layout.masked_files.contains(dest);
            if state.hidden(dest) && !masked {
                if enclosing == Region::Stub {
                    continue;
                }
                let op = if candidate.kind == NodeKind::Dir {
                    MountOp::EmptyDir { dest: dest.clone() }
                } else {
                    MountOp::FileStub { dest: dest.clone() }
                };
                emitter.insert(dest, op, Region::Stub);
                continue;
            }
            let Some(source) = &candidate.source else {
                continue;
            };
            let writable = state.writable(self.layout, dest);
            let differs = match enclosing {
                Region::Stub => true,
                Region::Mount { writable: outer } => outer != writable,
            };
            if candidate.is_base || differs {
                emitter.insert(
                    dest,
                    MountOp::Bind {
                        source: source.clone(),
                        dest: dest.clone(),
                        access: MountAccess::from_writable(writable),
                    },
                    Region::Mount { writable },
                );
            }
        }

        for (name, found) in restores {
            if !state.hidden(&name) {
                continue;
            }
            let overlaps = state.deny_read.iter().any(|deny| {
                (found.canon.is_within(&deny.path) || deny.path.is_within(&found.canon))
                    && !name.is_within(&deny.path)
                    && !deny.lifted_by.iter().any(|allow| deny.path.is_within(allow))
            });
            let Some(source) = found.source.host_source().filter(|_| !overlaps) else {
                suppressed.push(SuppressedEntry {
                    list: FsPolicyList::AllowReadWithinDeny,
                    path: found.canon,
                    reason: SuppressedReason::ViaSymlink,
                });
                continue;
            };
            emitter.insert(
                &name,
                MountOp::Bind {
                    source,
                    dest: name.clone(),
                    access: MountAccess::ReadOnly,
                },
                Region::Mount { writable: false },
            );
        }

        let mut placeholders = Vec::new();
        let mut unguarded = Vec::new();
        let absent = deny_write
            .absent
            .iter()
            .map(|path| (path, true))
            .chain(deny_read.absent.iter().map(|path| (path, false)));
        for (guard, must_guard) in absent {
            if emitter.contains(guard) {
                continue;
            }
            let Some(parent) = guard.parent() else {
                continue;
            };
            if state.hidden(&parent) || !state.writable(self.layout, guard) || emitter.enclosing(guard) == Region::Stub
            {
                continue;
            }
            let creatable = self
                .layout
                .host_path(&parent)
                .is_some_and(|host| self.tree.can_create_in(&host));
            match self.layout.host_path(guard) {
                Some(host) if creatable => {
                    placeholders.push(host);
                    emitter.insert(guard, MountOp::FileStub { dest: guard.clone() }, Region::Stub);
                }
                _ if must_guard => return Err(FsPlanError::UnguardableDenyWrite(guard.clone())),
                _ => unguarded.push(guard.clone()),
            }
        }

        self.pin_ancestors(&mut emitter)?;

        let mut ops: Vec<MountOp> = emitter.ops.into_values().collect();
        let stub_dirs: Vec<LiteralPath> = ops
            .iter()
            .filter_map(|op| match op {
                MountOp::EmptyDir { dest } => Some(dest.clone()),
                _ => None,
            })
            .collect();
        ops.extend(stub_dirs.into_iter().map(|dest| MountOp::RemountReadOnly { dest }));

        let total: usize = ops.iter().map(MountOp::arguments).sum();
        if total > MAX_BWRAP_ARGUMENTS - RESERVED_BWRAP_ARGUMENTS {
            let mandatory = ops
                .iter()
                .filter(|op| {
                    let dest = op.dest();
                    mandatory_paths
                        .iter()
                        .any(|path| dest.is_within(path) || path.is_within(dest))
                })
                .map(MountOp::arguments)
                .sum();
            return Err(FsPlanError::TooManyArguments {
                total,
                mandatory,
                configured: total - mandatory,
            });
        }

        Ok(MountPlan {
            ops,
            tmp_writable,
            placeholders,
            unguarded,
            suppressed,
            mandatory: mandatory_paths.into_iter().collect(),
        })
    }

    fn classify(&self, path: &LiteralPath, out: &mut Targets) -> Result<(), FsPlanError> {
        match self.resolve(path)? {
            Outcome::Found(found) => {
                out.found.push(found);
            }
            Outcome::Absent { guard } => out.absent.push(guard),
            Outcome::Held(found) => out.held.push(found),
            Outcome::Private | Outcome::Unreachable => {}
        }
        Ok(())
    }

    /// Where the mandatory list applies: the paths at the top of the working
    /// directory and every occurrence of a protected name below it, down to
    /// the search depth. A working
    /// directory outside every writable root has nothing to protect. Without
    /// a working directory the outermost writable roots stand in for it.
    fn mandatory_targets(&self, grants: &[Found], state: &State) -> Result<Targets, FsPlanError> {
        let mut out = Targets::default();
        let writable_dirs: BTreeSet<LiteralPath> = grants
            .iter()
            .filter(|grant| grant.source.kind() == NodeKind::Dir)
            .map(|grant| grant.canon.clone())
            .collect();
        let roots: BTreeSet<LiteralPath> = match &self.layout.working_dir {
            Some(cwd) => match self.resolve(cwd)? {
                Outcome::Found(found)
                    if found.source.kind() == NodeKind::Dir
                        && writable_dirs.iter().any(|dir| found.canon.is_within(dir)) =>
                {
                    BTreeSet::from([found.canon])
                }
                _ => BTreeSet::new(),
            },
            None => writable_dirs,
        };
        for root in &roots {
            for relative in mandatory::ROOT_PATHS {
                self.classify(&child(root, relative)?, &mut out)?;
            }
        }
        let mut visited = 0usize;
        for root in roots
            .iter()
            .filter(|root| !roots.iter().any(|outer| outer != *root && root.is_within(outer)))
        {
            self.scan(root, state, &mut visited, &mut out)?;
        }
        Ok(out)
    }

    fn scan(
        &self,
        root: &LiteralPath,
        state: &State,
        visited: &mut usize,
        out: &mut Targets,
    ) -> Result<(), FsPlanError> {
        let mut stack = vec![(root.clone(), false, 0u32)];
        while let Some((dir, known_git_dir, depth)) = stack.pop() {
            if state.hidden(&dir) {
                continue;
            }
            let mark = stack.len();
            match self.scan_dir(root, &dir, known_git_dir, depth, visited, out, &mut stack) {
                Err(FsPlanError::Host(HostTreeError::Io { source, .. }))
                    if source.kind() == io::ErrorKind::PermissionDenied =>
                {
                    stack.truncate(mark);
                    self.deny_unreadable(&dir, out)?;
                }
                other => other?,
            }
        }
        Ok(())
    }

    /// A directory the worker cannot list holds paths the scan cannot see, so
    /// the whole directory is denied instead of being skipped.
    fn deny_unreadable(&self, dir: &LiteralPath, out: &mut Targets) -> Result<(), FsPlanError> {
        if let Outcome::Found(found) = self.resolve(dir)? {
            out.unreadable.push(found);
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the scan carries its walk state as separate arguments"
    )]
    fn scan_dir(
        &self,
        root: &LiteralPath,
        dir: &LiteralPath,
        known_git_dir: bool,
        depth: u32,
        visited: &mut usize,
        out: &mut Targets,
        stack: &mut Vec<(LiteralPath, bool, u32)>,
    ) -> Result<(), FsPlanError> {
        let limit = self
            .options
            .mandatory_deny_search_depth
            .unwrap_or(MandatoryDenySearchDepth::DEFAULT);
        let names = self.list_dir(dir)?;
        if known_git_dir || self.looks_like_git_dir(dir, &names)? {
            if search_depth_reaches_directory(limit, depth) {
                self.protect_git_dir(dir, out)?;
            }
            let modules = child(dir, "modules")?;
            if self
                .source_of(&modules)?
                .is_some_and(|source| source.kind() == NodeKind::Dir)
            {
                stack.push((modules, false, depth + 1));
            }
            return Ok(());
        }
        for name in names {
            *visited += 1;
            if *visited > MAX_MANDATORY_VISITS {
                return Err(FsPlanError::MandatoryScanTooLarge { root: root.clone() });
            }
            let path = child(dir, &name)?;
            if self.layout.is_private(&path) {
                continue;
            }
            let child_depth = depth + 1;
            let file_reached = search_depth_reaches_file(limit, child_depth);
            let directory_reached = search_depth_reaches_directory(limit, child_depth);
            if mandatory::is_dangerous_file(&name) && file_reached
                || mandatory::is_dangerous_directory(&name) && directory_reached
            {
                self.classify(&path, out)?;
                continue;
            }
            if mandatory::is_dot_git(&name) {
                if directory_reached {
                    self.protect_dot_git(dir, &path, out, stack, child_depth, dir == root)?;
                }
                continue;
            }
            if mandatory::is_dot_agent(&name) && directory_reached {
                self.protect_dot_agent(&path, out)?;
            }
            if directory_reached
                && self
                    .source_of(&path)?
                    .is_some_and(|source| source.kind() == NodeKind::Dir)
            {
                stack.push((path, false, child_depth));
            }
        }
        Ok(())
    }

    fn kind_of(&self, path: &LiteralPath) -> Result<Option<NodeKind>, FsPlanError> {
        Ok(self.source_of(path)?.map(|source| source.kind()))
    }

    /// A git directory is known by its `HEAD`, `objects` and `refs`, which
    /// finds bare repositories and submodule directories that no `.git` names.
    fn looks_like_git_dir(&self, dir: &LiteralPath, names: &BTreeSet<String>) -> Result<bool, FsPlanError> {
        if !["HEAD", "objects", "refs"].iter().all(|name| names.contains(*name)) {
            return Ok(false);
        }
        Ok(self.kind_of(&child(dir, "HEAD")?)? == Some(NodeKind::File)
            && self.kind_of(&child(dir, "objects")?)? == Some(NodeKind::Dir)
            && self.kind_of(&child(dir, "refs")?)? == Some(NodeKind::Dir))
    }

    fn protect_git_dir(&self, dir: &LiteralPath, out: &mut Targets) -> Result<(), FsPlanError> {
        for name in mandatory::git_dir_protected(self.options.allow_git_config.is_writable()) {
            self.classify(&child(dir, name)?, out)?;
        }
        Ok(())
    }

    fn protect_dot_agent(&self, path: &LiteralPath, out: &mut Targets) -> Result<(), FsPlanError> {
        let Outcome::Found(found) = self.resolve(path)? else {
            return Ok(());
        };
        if found.source.kind() != NodeKind::Dir {
            return Ok(());
        }
        for name in self.list_dir(&found.canon)? {
            if mandatory::is_dot_agent_protected(&name) {
                self.classify(&child(&found.canon, &name)?, out)?;
            }
        }
        Ok(())
    }

    /// A `.git` directory protects its hooks and config. A `.git` file of the
    /// working directory itself is protected, so it stays a pointer to the directory it names, and
    /// the directory it names is protected when it can be reached from the
    /// guest. A pointer to a path the guest does not have leaves nothing for
    /// the sandbox to write. One below the working directory is left to git,
    /// which rewrites it when a checkout recurses into submodules.
    fn protect_dot_git(
        &self,
        parent: &LiteralPath,
        path: &LiteralPath,
        out: &mut Targets,
        stack: &mut Vec<(LiteralPath, bool, u32)>,
        depth: u32,
        own: bool,
    ) -> Result<(), FsPlanError> {
        let Outcome::Found(found) = self.resolve(path)? else {
            return Ok(());
        };
        match found.source.kind() {
            NodeKind::Dir => stack.push((found.canon, true, depth)),
            NodeKind::File if !own => {}
            NodeKind::File => {
                let host = match &found.source {
                    Source::Host { path, .. } => path.clone(),
                    Source::Synthetic => return Ok(()),
                };
                out.found.push(found);
                self.protect_gitfile_target(parent, &host, out)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn read_pointer(&self, host: &Path) -> Result<Option<Vec<u8>>, FsPlanError> {
        Ok(self.tree.read_file(host, MAX_GIT_POINTER_BYTES)?)
    }

    fn resolve_pointer(&self, base: &LiteralPath, pointer: &str) -> Result<Outcome, FsPlanError> {
        let mut pending: VecDeque<String> = VecDeque::new();
        if !pointer.starts_with('/') {
            pending.extend(base.components().map(str::to_string));
        }
        pending.extend(
            pointer
                .split('/')
                .filter(|component| !component.is_empty())
                .map(str::to_string),
        );
        self.resolve_components(base, pending)
    }

    fn protect_gitfile_target(&self, parent: &LiteralPath, host: &Path, out: &mut Targets) -> Result<(), FsPlanError> {
        let Some(content) = self.read_pointer(host)? else {
            return Ok(());
        };
        let Some(pointer) = mandatory::gitdir_pointer(&content) else {
            return Ok(());
        };
        let git_dir = match self.resolve_pointer(parent, pointer)? {
            Outcome::Found(found) if found.source.kind() == NodeKind::Dir => found.canon,
            Outcome::Absent { guard } => {
                out.absent.push(guard);
                return Ok(());
            }
            _ => return Ok(()),
        };
        self.protect_git_dir(&git_dir, out)?;

        let commondir = child(&git_dir, "commondir")?;
        let Some(Source::Host { path: host, node }) = self.source_of(&commondir)? else {
            return Ok(());
        };
        if node.kind != NodeKind::File {
            return Ok(());
        }
        let Some(content) = self.read_pointer(&host)? else {
            return Ok(());
        };
        let Some(pointer) = mandatory::commondir_pointer(&content) else {
            return Ok(());
        };
        if let Outcome::Found(common) = self.resolve_pointer(&git_dir, pointer)?
            && common.source.kind() == NodeKind::Dir
        {
            self.protect_git_dir(&common.canon, out)?;
        }
        Ok(())
    }

    fn refuse_unenforceable_write_globs(&self, policy: &FsPolicy, state: &State) -> Result<(), FsPlanError> {
        for entry in policy.deny_write() {
            let FsPathEntry::Glob { pattern } = entry else {
                continue;
            };
            let lexical = GlobMatcher::new(pattern).static_base()?;
            let region = match self.resolve(&lexical)? {
                Outcome::Found(found) => found.canon,
                _ => lexical.clone(),
            };
            let related = |a: &LiteralPath, b: &LiteralPath| a.is_within(b) || b.is_within(a);
            if state
                .grants
                .iter()
                .any(|grant| related(grant, &lexical) || related(grant, &region))
            {
                return Err(FsPlanError::UnenforceableWriteGlob {
                    pattern: pattern.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Makes every directory above a protected path a mount point, so renaming
    /// or removing it fails with EBUSY instead of moving the protection away.
    /// The pins are then covered by a second bind of the writable mount that
    /// holds them (a top-level directory under a root write grant), so a
    /// rename or link between two of them stays on one mount.
    fn pin_ancestors(&self, emitter: &mut Emitter) -> Result<(), FsPlanError> {
        let bury = self.layout.host_view;
        let root_writable = bury && emitter.regions.get(&root_path()?) == Some(&Region::Mount { writable: true });
        let kernel_dirs = ["/proc", "/dev", "/sys"]
            .into_iter()
            .map(LiteralPath::new)
            .collect::<Result<Vec<_>, _>>()?;
        let protections: Vec<LiteralPath> = emitter
            .ops
            .iter()
            .filter(|(_, op)| match op {
                MountOp::EmptyDir { .. } | MountOp::FileStub { .. } => true,
                MountOp::Bind { access, .. } => !access.is_writable(),
                _ => false,
            })
            .map(|((_, _, dest), _)| dest.clone())
            .filter(|dest| !(root_writable && kernel_dirs.iter().any(|dir| dest.is_within(dir))))
            .collect();
        let mut pins: BTreeMap<LiteralPath, usize> = BTreeMap::new();
        let mut covers = BTreeSet::new();
        for protection in protections {
            let mut cover = None;
            let mut chain = Vec::new();
            let mut anchor = None;
            for ancestor in protection.ancestors().skip(1) {
                if ancestor.is_root() {
                    break;
                }
                if emitter.contains(&ancestor) {
                    if emitter.regions.get(&ancestor) != Some(&Region::Mount { writable: true }) {
                        break;
                    }
                    if !bury {
                        break;
                    }
                    if root_writable {
                        cover = Some(ancestor);
                    } else {
                        anchor = Some(depth(&ancestor));
                    }
                    continue;
                }
                if emitter.enclosing(&ancestor) != (Region::Mount { writable: true }) {
                    break;
                }
                if root_writable && ancestor.parent().is_some_and(|parent| parent.is_root()) {
                    cover = Some(ancestor);
                } else {
                    chain.push(ancestor);
                }
            }
            for pin in chain {
                let at = anchor.unwrap_or_else(|| depth(&pin));
                let slot = pins.entry(pin).or_insert(at);
                *slot = (*slot).min(at);
            }
            covers.extend(cover);
        }
        let mut binds = Vec::new();
        for (dir, anchor) in pins {
            binds.push((dir, Some(anchor)));
        }
        for dir in &covers {
            binds.push((dir.clone(), None));
        }
        for (dir, anchor) in binds {
            let Some(Source::Host { path, node }) = self.source_of(&dir)? else {
                continue;
            };
            if node.kind != NodeKind::Dir {
                continue;
            }
            let op = MountOp::Bind {
                source: HostSource {
                    path,
                    id: node.id,
                    kind: NodeKind::Dir,
                },
                dest: dir.clone(),
                access: MountAccess::ReadWrite,
            };
            let region = Region::Mount { writable: true };
            match anchor {
                None => emitter.insert_in(Step::Cover, &dir, op, region),
                Some(_) if !bury => emitter.insert(&dir, op, region),
                Some(_) if root_writable => emitter.insert_in(Step::Pin, &dir, op, region),
                Some(anchor) => emitter.insert_beneath(anchor, &dir, op, region),
            }
        }
        let held: Vec<_> = emitter
            .ops
            .keys()
            .filter(|(step, _, dest)| {
                *step == Step::Plain as u8 && covers.iter().any(|cover| dest.is_within(cover) && dest != cover)
            })
            .cloned()
            .collect();
        for key in held {
            if let Some(op) = emitter.ops.remove(&key) {
                emitter.ops.insert((Step::Covered as u8, key.1, key.2), op);
            }
        }
        Ok(())
    }
}
