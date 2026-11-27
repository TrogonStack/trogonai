//! The macOS isolation backend: the payload runs under `sandbox-exec` with a
//! Seatbelt profile generated per launch.
//!
//! What it enforces is weaker than the Linux backend, and the profile the
//! worker advertises says only what holds here:
//!
//! - There is no network namespace. A payload limited to the egress proxy is
//!   limited by a profile rule that allows connecting to one loopback port,
//!   and any other local process can reach that port too; the lease token is
//!   all that protects it. Without the proxy the network is all or nothing.
//! - There is no mount namespace. The payload sees the host file tree, rules
//!   are written against host paths, and a guest path of the policy
//!   (`/workspace/...`) is rewritten to where the workspace really is.
//! - Rules are path based. A protected path cannot be moved away because the
//!   profile also denies removing it and the directories that lead to it, but
//!   a hard link made before the launch still reaches the same data.
//! - There is no seccomp filter, PID namespace or resource limit.
//! - `sandbox-exec` is deprecated by Apple, though still shipped and used by
//!   Apple's own tools.
//! - Masked files and anything else the profile cannot express are denied
//!   rather than substituted.

mod baseline;
mod compose;
mod monitor;
mod opt_ins;
mod policy;
pub mod sbpl;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use nix::sys::signal;
use nix::unistd::Pid;
use rand_core::{OsRng, RngCore};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use trogon_sandbox_contract::{DirectoryMarker, FsPathEntry, FsPolicy, FsPolicyError, LiteralPath, PtyAccess};
use trogon_sandbox_protocol::LiteralPathExt as _;
use trogon_std::env::{ReadEnv, SystemEnv};

use self::compose::compose_with_opt_ins;
use self::monitor::ViolationMonitor;
use self::policy::{PathMapper, host_path};
use self::sbpl::{LogTag, Profile};
use super::default_writes::{RUNTIME_MARKER_VALUE, RUNTIME_MARKER_VAR};
use super::exit_cleanup::ExitGuard;
use super::host_view::HostView;
use super::limits::LimitGuard;
use super::{ChildStdio, IsolationBackend, LaunchedProcess, TerminalStdio};
use crate::subprocess::{ExtraBind, LaunchSpec};

pub use self::monitor::{SeatbeltViolation, ViolationKind};

pub struct SeatbeltBackend {
    sandbox_exec: PathBuf,
    violations: Option<UnboundedSender<SeatbeltViolation>>,
    stdio: ChildStdio,
    host_view: Option<HostView>,
}

impl SeatbeltBackend {
    pub fn new() -> Self {
        Self {
            sandbox_exec: PathBuf::from(SANDBOX_EXEC),
            violations: None,
            stdio: ChildStdio::default(),
            host_view: None,
        }
    }

    /// Reports what the kernel denies to `sink`. Reading denials back from the
    /// unified log is best effort: the stream can start after the first
    /// denials of a launch, and nothing here depends on it.
    pub fn with_violation_sink(mut self, sink: UnboundedSender<SeatbeltViolation>) -> Self {
        self.violations = Some(sink);
        self
    }

    pub fn with_stdio(mut self, stdio: ChildStdio) -> Self {
        self.stdio = stdio;
        self
    }

    /// Runs the command in the view's working directory with the environment
    /// of the lease as it is. A lease without an fs policy then reads
    /// everything and writes nothing, instead of getting the curated default.
    /// Policy paths are host paths on this platform either way.
    pub fn with_host_view(mut self, view: HostView) -> Self {
        self.host_view = Some(view);
        self
    }
}

impl Default for SeatbeltBackend {
    fn default() -> Self {
        Self::new()
    }
}

struct Scratch {
    _dir: tempfile::TempDir,
    path: LiteralPath,
    _exit: ExitGuard,
}

impl Scratch {
    fn create() -> anyhow::Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("sandbox-seatbelt-")
            .tempdir()
            .context("creating the sandbox scratch directory")?;
        let path = host_path(dir.path())?;
        let exit = ExitGuard::hold(dir.path());
        Ok(Self {
            _dir: dir,
            path,
            _exit: exit,
        })
    }
}

fn profile_argument(
    profile: &Profile,
) -> anyhow::Result<(&'static str, std::ffi::OsString, Option<tempfile::NamedTempFile>)> {
    if profile.as_str().len() <= INLINE_PROFILE_BYTES {
        return Ok(("-p", profile.as_str().into(), None));
    }
    let mut file = tempfile::Builder::new()
        .prefix("sandbox-profile-")
        .tempfile()
        .context("writing the sandbox profile")?;
    std::io::Write::write_all(&mut file, profile.as_str().as_bytes()).context("writing the sandbox profile")?;
    Ok(("-f", file.path().as_os_str().to_owned(), Some(file)))
}

fn log_tag() -> anyhow::Result<LogTag> {
    let mut bytes = [0u8; TAG_RANDOM_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(LogTag::new(format!("{TAG_PREFIX}{hex}"))?)
}

fn guest_path(path: &Path) -> anyhow::Result<LiteralPath> {
    let raw = path
        .to_str()
        .with_context(|| format!("{} is not valid UTF-8", path.display()))?;
    LiteralPath::new(raw).with_context(|| format!("{raw} is not a usable path"))
}

fn literal(path: &LiteralPath) -> FsPathEntry {
    FsPathEntry::Literal {
        path: path.clone(),
        directory: DirectoryMarker::Unmarked,
    }
}

struct Layout {
    workspace: LiteralPath,
    binds: Vec<(LiteralPath, ExtraBind)>,
    mapper: PathMapper,
}

impl Layout {
    fn new(spec: &LaunchSpec) -> anyhow::Result<Self> {
        let workspace = guest_path(&spec.workspace_sandbox_path)?;
        let mut mounts = vec![(workspace.clone(), host_path(&spec.workspace_host_path)?)];
        let mut binds = Vec::new();
        for bind in &spec.extra_binds {
            let guest = LiteralPath::new(&bind.sandbox_path)
                .with_context(|| format!("{} is not a usable path", bind.sandbox_path))?;
            mounts.push((guest.clone(), host_path(&bind.host_path)?));
            binds.push((guest, bind.clone()));
        }
        Ok(Self {
            workspace,
            binds,
            mapper: PathMapper::new(mounts),
        })
    }
}

/// The policy the profile is built from, in guest paths: the lease's own when
/// it has one, otherwise the default that keeps the payload out of the home
/// directory of whoever runs the worker and lets it write only where it
/// works.
fn effective_policy(
    spec: &LaunchSpec,
    layout: &Layout,
    scratch: &LiteralPath,
    home: Option<LiteralPath>,
    host_view: bool,
) -> anyhow::Result<FsPolicy> {
    let mut lists = match &spec.fs_policy {
        Some(policy) => PolicyLists::of(policy),
        None if host_view => PolicyLists::default(),
        None => {
            let mut lists = PolicyLists::default();
            lists.deny_read.extend(home.as_ref().map(literal));
            lists.allow_read_within_deny.push(literal(&layout.workspace));
            lists
                .allow_read_within_deny
                .extend(layout.binds.iter().map(|(guest, _)| literal(guest)));
            lists.allow_read_within_deny.push(literal(scratch));
            if !spec.workspace_readonly {
                lists.allow_write.push(literal(&layout.workspace));
            }
            lists.allow_write.extend(
                layout
                    .binds
                    .iter()
                    .filter(|(_, bind)| !bind.readonly)
                    .map(|(guest, _)| literal(guest)),
            );
            lists
        }
    };
    lists.allow_write.push(literal(scratch));
    if spec.workspace_readonly {
        lists.deny_write.push(literal(&layout.workspace));
    }
    lists.deny_write.extend(
        layout
            .binds
            .iter()
            .filter(|(_, bind)| bind.readonly)
            .map(|(guest, _)| literal(guest)),
    );
    Ok(lists.into_policy()?)
}

/// The six lists of a policy, open to extension before the policy is rebuilt.
#[derive(Default)]
struct PolicyLists {
    deny_read: Vec<FsPathEntry>,
    credential_deny_read: Vec<FsPathEntry>,
    allow_read_within_deny: Vec<FsPathEntry>,
    own_allow_within_deny: Vec<FsPathEntry>,
    allow_write: Vec<FsPathEntry>,
    deny_write: Vec<FsPathEntry>,
}

impl PolicyLists {
    fn of(policy: &FsPolicy) -> Self {
        Self {
            deny_read: policy.deny_read().to_vec(),
            credential_deny_read: policy.credential_deny_read().to_vec(),
            allow_read_within_deny: policy.allow_read_within_deny().to_vec(),
            own_allow_within_deny: policy.own_allow_within_deny().to_vec(),
            allow_write: policy.allow_write().to_vec(),
            deny_write: policy.deny_write().to_vec(),
        }
    }

    fn into_policy(self) -> Result<FsPolicy, FsPolicyError> {
        FsPolicy::new(
            self.deny_read,
            self.credential_deny_read,
            self.allow_read_within_deny,
            self.own_allow_within_deny,
            self.allow_write,
            self.deny_write,
        )
    }
}

fn home_directory(env: &impl ReadEnv) -> Option<LiteralPath> {
    let home = env.var_os("HOME")?;
    let resolved = host_path(Path::new(&home)).ok()?;
    (!resolved.is_root()).then_some(resolved)
}

fn sandbox_env(spec: &LaunchSpec, scratch: &LiteralPath, host_view: Option<&HostView>) -> Vec<(String, String)> {
    if host_view.is_some() {
        return spec.env.clone();
    }
    let mut env = std::collections::BTreeMap::from([
        ("PATH".to_string(), DEFAULT_PATH.to_string()),
        (
            "HOME".to_string(),
            spec.workspace_host_path.to_string_lossy().into_owned(),
        ),
        ("TMPDIR".to_string(), scratch.as_str().to_string()),
    ]);
    env.extend(spec.env.iter().cloned());
    env.insert(RUNTIME_MARKER_VAR.to_string(), RUNTIME_MARKER_VALUE.to_string());
    env.into_iter().collect()
}

fn build_profile(
    spec: &LaunchSpec,
    layout: &Layout,
    scratch: &LiteralPath,
    home: Option<LiteralPath>,
    tag: LogTag,
    host_view: bool,
) -> anyhow::Result<Profile> {
    let policy = effective_policy(spec, layout, scratch, home, host_view)?;
    compose_with_opt_ins(
        &policy,
        &layout.mapper,
        &layout.workspace,
        &spec.egress_policy,
        &spec.platform.macos,
        &spec.platform.filesystem,
        tag,
    )
}

fn is_running(pid: u32) -> bool {
    i32::try_from(pid).is_ok_and(|pid| signal::kill(Pid::from_raw(pid), None).is_ok())
}

#[async_trait]
impl IsolationBackend for SeatbeltBackend {
    async fn launch(&self, spec: &LaunchSpec) -> anyhow::Result<LaunchedProcess> {
        self.spawn(spec, None).await
    }

    async fn launch_terminal(&self, spec: &LaunchSpec, tty: &TerminalStdio) -> anyhow::Result<LaunchedProcess> {
        // The profile denies terminal ioctls unless it allows ptys, and a
        // terminal is what this launch is for. The lease's own process keeps
        // whatever it asked for.
        let mut spec = spec.clone();
        spec.platform.macos.allow_pty = PtyAccess::Allowed;
        self.spawn(&spec, Some(tty)).await
    }
}

impl SeatbeltBackend {
    async fn spawn(&self, spec: &LaunchSpec, tty: Option<&TerminalStdio>) -> anyhow::Result<LaunchedProcess> {
        anyhow::ensure!(
            spec.user.is_none(),
            "the seatbelt backend cannot run a payload as another user"
        );
        super::platform_support::ensure_enforceable(&spec.platform, true, false)?;
        let scratch = Scratch::create()?;
        let layout = Layout::new(spec)?;
        let profile = build_profile(
            spec,
            &layout,
            &scratch.path,
            home_directory(&SystemEnv),
            log_tag()?,
            self.host_view.is_some(),
        )?;

        let monitor = (self.violations.is_some() || spec.fs_denials.is_some())
            .then(|| ViolationMonitor::start(profile.tag(), self.violations.clone(), spec.fs_denials.clone()))
            .flatten();

        let (profile_flag, profile_arg, profile_file) = profile_argument(&profile)?;
        let mut cmd = Command::new(&self.sandbox_exec);
        cmd.arg(profile_flag)
            .arg(profile_arg)
            .arg(&spec.binary)
            .args(&spec.args)
            .env_clear()
            .current_dir(match &self.host_view {
                Some(view) => view.cwd(),
                None => &spec.workspace_host_path,
            });
        for (key, value) in sandbox_env(spec, &scratch.path, self.host_view.as_ref()) {
            cmd.env(key, value);
        }

        self.stdio.apply(&mut cmd);
        if let Some(tty) = tty
            && let Err(error) = tty.apply(&mut cmd)
        {
            if let Some(monitor) = monitor {
                monitor.stop().await;
            }
            return Err(error).context("attaching the terminal");
        }
        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                if let Some(monitor) = monitor {
                    monitor.stop().await;
                }
                return Err(error).with_context(|| format!("starting {}", self.sandbox_exec.display()));
            }
        };
        let pid = child.id().unwrap_or(0);

        tokio::spawn(async move {
            while pid != 0 && is_running(pid) {
                tokio::time::sleep(LIVENESS_POLL).await;
            }
            if let Some(monitor) = monitor {
                tokio::time::sleep(MONITOR_GRACE).await;
                monitor.stop().await;
            }
            drop(scratch);
            drop(profile_file);
        });

        Ok(LaunchedProcess {
            pid,
            child: Arc::new(Mutex::new(Some(child))),
            limits: LimitGuard::default(),
            placeholders: Vec::new(),
            unguarded: Vec::new(),
            monitors: Default::default(),
        })
    }
}

mod constants;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
use constants::{
    DEFAULT_PATH, INLINE_PROFILE_BYTES, LIVENESS_POLL, MONITOR_GRACE, SANDBOX_EXEC, TAG_PREFIX, TAG_RANDOM_BYTES,
};
