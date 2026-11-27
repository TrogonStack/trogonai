use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Seek, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::Context;
use async_trait::async_trait;
use nix::fcntl::OFlag;
use nix::libc;
use tokio::process::Command;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{DirectoryMarker, FsPathEntry, FsPolicy, LiteralPath, LiteralPathError};

use super::default_writes::{RUNTIME_MARKER_VALUE, RUNTIME_MARKER_VAR};
use super::guest_init::InitWiring;
use super::host_tree::{LinuxHostTree, open_source};
use super::host_view::{HostView, host_layout};
use super::landlock::{Confinement, GUEST_LAUNCHER_BINARY, LandlockSupport, WritePlan};
use super::limits::ResourceEnforcer;
use super::mount_plan::{GuestLayout, MountAccess, MountPlan, plan_mounts_with};
use super::nested;
use super::observer::{ObserverWiring, WriteMap};
use super::relay::{GUEST_RELAY_BINARY, RelayWiring, open_path};
use super::seccomp::{SeccompOptions, build_sandbox_filters};
use super::userns::{NamespaceGuard, can_disable_userns, capability_args, holds_setfcap};
use super::{ChildStdio, IsolationBackend, LaunchedProcess, MonitorGuard, TerminalStdio, share_network};
use crate::subprocess::{EgressPolicy, LaunchSpec};

use super::constants::{DEFAULT_PATH, FD_HEADROOM, ROOT_SYSTEM_DIRS};

pub struct BubblewrapBackend {
    bwrap_path: PathBuf,
    limits: ResourceEnforcer,
    second_layer: SecondLayer,
    observer_binary: Option<PathBuf>,
    guest_init: Option<PathBuf>,
    stdio: ChildStdio,
    host_view: Option<HostView>,
    can_disable_userns: OnceLock<bool>,
}

/// How a lease with an fs policy is confined a second time by Landlock.
struct SecondLayer {
    support: LandlockSupport,
    launcher: Option<PathBuf>,
    writes: Option<WritePlan>,
}

impl SecondLayer {
    fn detect() -> Self {
        let support = LandlockSupport::detect();
        support.log();
        Self {
            support,
            launcher: None,
            writes: None,
        }
    }

    fn confinement(&self, plan: &MountPlan) -> Result<Option<Confinement>, LiteralPathError> {
        let Some(abi) = self.support.abi() else {
            return Ok(None);
        };
        let writes = match &self.writes {
            Some(writes) => writes.clone(),
            None => WritePlan::from_mounts(plan)?,
        };
        Ok(Some(Confinement::new(abi, writes)))
    }

    fn launcher_binary(&self) -> anyhow::Result<PathBuf> {
        match &self.launcher {
            Some(path) => Ok(path.clone()),
            None => std::env::current_exe().context("locating the confinement launcher"),
        }
    }
}

impl BubblewrapBackend {
    pub fn new(bwrap_path: PathBuf) -> Self {
        Self::with_resource_enforcer(bwrap_path, ResourceEnforcer::detect())
    }

    pub fn with_resource_enforcer(bwrap_path: PathBuf, limits: ResourceEnforcer) -> Self {
        Self {
            bwrap_path,
            limits,
            second_layer: SecondLayer::detect(),
            observer_binary: None,
            guest_init: None,
            stdio: ChildStdio::default(),
            host_view: None,
            can_disable_userns: OnceLock::new(),
        }
    }

    fn namespace_guard(&self, spec: &LaunchSpec) -> NamespaceGuard {
        let linux = &spec.platform.linux;
        let can_disable = linux.allow_all_unix_sockets.is_allowed()
            && *self
                .can_disable_userns
                .get_or_init(|| can_disable_userns(&self.bwrap_path));
        NamespaceGuard::decide(linux, can_disable)
    }

    /// The binary that reports filesystem violations from inside the sandbox
    /// when the lease asks for them. It must handle `--observe` the way the
    /// worker binary does. Defaults to the running executable.
    pub fn with_observer(mut self, binary: PathBuf) -> Self {
        self.observer_binary = Some(binary);
        self
    }

    fn observer_binary(&self) -> anyhow::Result<PathBuf> {
        match &self.observer_binary {
            Some(path) => Ok(path.clone()),
            None => std::env::current_exe().context("locating the observer binary"),
        }
    }

    /// The binary that applies the Landlock ruleset inside the sandbox when
    /// the lease has no egress relay. It must handle `--confine` the way the
    /// worker binary does. Defaults to the running executable.
    pub fn with_confiner(mut self, launcher: PathBuf) -> Self {
        self.second_layer.launcher = Some(launcher);
        self
    }

    /// Runs the sandbox's first process from this binary, which must handle
    /// `--guest-init` the way the worker binary does. It keeps PID 1 from
    /// being dumpable, so the payload cannot reach it through `/proc`.
    pub fn with_guest_init(mut self, binary: PathBuf) -> Self {
        self.guest_init = Some(binary);
        self
    }

    pub fn with_stdio(mut self, stdio: ChildStdio) -> Self {
        self.stdio = stdio;
        self
    }

    /// Mounts the host's root read-only instead of the fixed guest layout,
    /// starts the command in the view's working directory and passes it the
    /// environment of the lease as it is. The lease's fs policy then decides
    /// what is writable or hidden; without one nothing is.
    pub fn with_host_view(mut self, view: HostView) -> Self {
        self.host_view = Some(view);
        self
    }

    pub fn with_landlock_support(mut self, support: LandlockSupport) -> Self {
        self.second_layer.support = support;
        self
    }

    /// Replaces the roots derived from the mount plan. Landlock then
    /// restricts to these roots whatever the mounts allow, which is how a
    /// mis-planned bind is simulated.
    pub fn with_write_plan(mut self, writes: WritePlan) -> Self {
        self.second_layer.writes = Some(writes);
        self
    }

    pub fn landlock_support(&self) -> LandlockSupport {
        self.second_layer.support
    }

    pub fn resource_enforcer(&self) -> &ResourceEnforcer {
        &self.limits
    }

    fn push_ro_bind(args: &mut Vec<String>, host: &Path, sandbox: &str) {
        if host.exists() {
            args.push("--ro-bind".to_string());
            args.push(host.to_string_lossy().into_owned());
            args.push(sandbox.to_string());
        }
    }

    /// On usr-merged hosts `/bin`, `/lib` and friends are symlinks into
    /// `/usr`. Recreating them as symlinks keeps paths like `/bin/sh`
    /// resolvable inside the sandbox.
    fn push_system_dir(args: &mut Vec<String>, path: &str) {
        let host = Path::new(path);
        match std::fs::symlink_metadata(host) {
            Ok(meta) if meta.file_type().is_symlink() => {
                if let Ok(target) = std::fs::read_link(host) {
                    args.push("--symlink".to_string());
                    args.push(target.to_string_lossy().into_owned());
                    args.push(path.to_string());
                }
            }
            Ok(_) => push_ro_bind(args, host, path),
            Err(_) => {}
        }
    }

    fn push_user(args: &mut Vec<String>, spec: &LaunchSpec) -> anyhow::Result<()> {
        if let Some(user) = &spec.user {
            let (uid, gid) = parse_uid_gid(user)?;
            args.push("--uid".to_string());
            args.push(uid.to_string());
            if let Some(gid) = gid {
                args.push("--gid".to_string());
                args.push(gid.to_string());
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn build_args(spec: &LaunchSpec, seccomp_fds: &[i32]) -> anyhow::Result<Vec<String>> {
        let guard = NamespaceGuard::decide(&spec.platform.linux, true);
        Self::assemble(spec, seccomp_fds, None, None, None, None, None, &guard)
    }

    /// With `mounts_fd` the filesystem is described by the argument file the
    /// descriptor points at, otherwise by the fixed layout below. With
    /// `relay` the network namespace is empty and the payload starts behind
    /// the relay, which installs the seccomp filters itself. With
    /// `confinement` the payload starts behind the Landlock launcher. With
    /// `observer` it starts behind the filesystem observer, which installs
    /// the seccomp filters itself when `seccomp_fds` is empty and there is no
    /// relay.
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is a distinct input of the one launch being assembled"
    )]
    fn assemble(
        spec: &LaunchSpec,
        seccomp_fds: &[i32],
        mounts_fd: Option<i32>,
        relay: Option<&RelayWiring>,
        confinement: Option<&Confinement>,
        observer: Option<&ObserverWiring>,
        init: Option<&InitWiring>,
        guard: &NamespaceGuard,
    ) -> anyhow::Result<Vec<String>> {
        let mut args = vec![
            "--unshare-all".to_string(),
            "--unshare-user".to_string(),
            "--new-session".to_string(),
        ];
        let nested = init.is_some() && guard.uses_helper();
        args.extend(capability_args(
            nested,
            nix::unistd::geteuid().is_root(),
            holds_setfcap(),
        ));
        args.extend(guard.args());

        if share_network(&spec.egress_policy) {
            args.push("--share-net".to_string());
        }
        if relay.is_some() {
            args.push("--unshare-net".to_string());
        }

        match mounts_fd {
            Some(fd) => {
                Self::push_user(&mut args, spec)?;
                args.push("--args".to_string());
                args.push(fd.to_string());
                args.push("--die-with-parent".to_string());
            }
            None => {
                push_ro_bind(&mut args, Path::new("/usr"), "/usr");
                for dir in ROOT_SYSTEM_DIRS {
                    Self::push_system_dir(&mut args, dir);
                }

                Self::push_user(&mut args, spec)?;

                if spec.workspace_readonly {
                    args.push("--ro-bind".to_string());
                } else {
                    args.push("--bind".to_string());
                }
                args.push(spec.workspace_host_path.to_string_lossy().into_owned());
                args.push(spec.workspace_sandbox_path.to_string_lossy().into_owned());

                for bind in &spec.extra_binds {
                    if bind.readonly {
                        args.push("--ro-bind".to_string());
                    } else {
                        args.push("--bind".to_string());
                    }
                    args.push(bind.host_path.to_string_lossy().into_owned());
                    args.push(bind.sandbox_path.clone());
                }

                args.extend(["--dev".to_string(), "/dev".to_string()]);
                if let Some(relay) = relay {
                    args.extend(relay.mount_args());
                }
                if let Some(observer) = observer {
                    args.extend(observer.mount_args());
                }
                if let Some(init) = init {
                    args.extend(init.mount_args());
                }

                args.extend(proc_mount_args(spec));
                args.extend([
                    "--tmpfs".to_string(),
                    "/tmp".to_string(),
                    "--die-with-parent".to_string(),
                ]);
            }
        }

        for fd in seccomp_fds {
            args.push("--add-seccomp-fd".to_string());
            args.push(fd.to_string());
        }

        if init.is_some() {
            args.push("--as-pid-1".to_string());
        }
        args.push("--".to_string());
        let (binary, payload_args) = match confinement {
            Some(confinement) => {
                let launcher = if relay.is_some() {
                    GUEST_RELAY_BINARY
                } else {
                    GUEST_LAUNCHER_BINARY
                };
                confinement.wrap_command(launcher, &spec.binary, &spec.args)
            }
            None => (spec.binary.clone(), spec.args.clone()),
        };
        let (binary, payload_args) = if nested {
            nested::wrap_command(&binary.to_string_lossy(), &payload_args, guard.seccomp())
        } else {
            (binary.to_string_lossy().into_owned(), payload_args)
        };
        let filters_in_nest = SeccompOptions {
            allow_unix_sockets: true,
            allow_nested_namespaces: true,
        };
        let (binary, payload_args) = match relay {
            Some(_) => RelayWiring::wrap_command(
                Path::new(&binary),
                &payload_args,
                if nested { filters_in_nest } else { guard.seccomp() },
            ),
            None => (binary, payload_args),
        };
        let (binary, payload_args) = match observer {
            Some(_) => ObserverWiring::wrap_command(
                Path::new(&binary),
                &payload_args,
                relay.is_none() && seccomp_fds.is_empty() && !nested,
                guard.seccomp(),
            ),
            None => (binary, payload_args),
        };
        let (binary, payload_args) = match init {
            Some(_) => InitWiring::wrap_command(&binary, &payload_args),
            None => (binary, payload_args),
        };
        args.push(binary);
        args.extend(payload_args);

        Ok(args)
    }
}

fn raise_fd_limit_for(binds: usize) -> anyhow::Result<()> {
    let needed = binds as u64 + FD_HEADROOM;
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(std::io::Error::last_os_error()).context("reading the descriptor limit");
    }
    let current: u64 = limit.rlim_cur;
    if current >= needed {
        return Ok(());
    }
    let hard: u64 = limit.rlim_max;
    anyhow::ensure!(
        hard >= needed,
        "the plan needs {needed} descriptors and the hard limit is {hard}"
    );
    limit.rlim_cur = needed as _;
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
        return Err(std::io::Error::last_os_error()).context("raising the descriptor limit");
    }
    Ok(())
}

/// The mounts of a lease that carries an fs policy. The descriptors stay open
/// until bubblewrap has been spawned.
struct PolicyMounts {
    plan: MountPlan,
    confinement: Option<Confinement>,
    launcher_fd: Option<OwnedFd>,
    bind_fds: Vec<OwnedFd>,
    args_fd: OwnedFd,
}

impl PolicyMounts {
    fn prepare(
        spec: &LaunchSpec,
        policy: &FsPolicy,
        relay: Option<&RelayWiring>,
        observer: Option<&ObserverWiring>,
        init: Option<&InitWiring>,
        second_layer: &SecondLayer,
        host_view: Option<&HostView>,
    ) -> anyhow::Result<Self> {
        let layout = layout_of(spec, host_view)?;
        let plan = plan_mounts_with(policy, &layout, &LinuxHostTree, &spec.platform.filesystem)?;

        let confinement = second_layer.confinement(&plan)?;
        if confinement.is_none() || !matches!(second_layer.support, LandlockSupport::Full(_)) {
            second_layer.support.log();
        }
        let launcher_fd = match (&confinement, relay) {
            (Some(_), None) => Some(open_path(&second_layer.launcher_binary()?, OFlag::empty())?),
            _ => None,
        };

        raise_fd_limit_for(plan.bind_sources().count())?;
        let bind_fds = plan.bind_sources().map(open_source).collect::<Result<Vec<_>, _>>()?;
        let raw: Vec<i32> = bind_fds.iter().map(|fd| fd.as_raw_fd()).collect();

        let mut args = plan.render(&raw)?;
        args.extend(["--dev", "/dev"].map(str::to_string));
        if let Some(relay) = relay {
            args.extend(relay.mount_args());
        }
        if let Some(observer) = observer {
            args.extend(observer.mount_args());
        }
        if let Some(init) = init {
            args.extend(init.mount_args());
        }
        if let Some(fd) = &launcher_fd {
            args.extend([
                "--ro-bind-fd".to_string(),
                fd.as_raw_fd().to_string(),
                GUEST_LAUNCHER_BINARY.to_string(),
            ]);
        }
        args.extend(proc_mount_args(spec));
        match host_view {
            Some(view) => {
                if !plan.root_writable() {
                    args.extend(["--remount-ro", "/"].map(str::to_string));
                }
                args.push("--chdir".to_string());
                args.push(view.cwd().to_string_lossy().into_owned());
            }
            None => {
                args.extend(["--tmpfs", "/tmp"].map(str::to_string));
                if !plan.tmp_writable {
                    args.extend(["--remount-ro", "/tmp"].map(str::to_string));
                }
                args.extend(["--remount-ro", "/"].map(str::to_string));
            }
        }

        let args_fd = memfd_with_args(&args)?;
        Ok(Self {
            plan,
            confinement,
            launcher_fd,
            bind_fds,
            args_fd,
        })
    }

    fn inherited_fds(&self) -> Vec<i32> {
        self.bind_fds
            .iter()
            .chain(self.launcher_fd.iter())
            .chain(std::iter::once(&self.args_fd))
            .map(|fd| fd.as_raw_fd())
            .collect()
    }
}

fn memfd_with_args(args: &[String]) -> anyhow::Result<OwnedFd> {
    let raw = unsafe { libc::memfd_create(c"bwrap-args".as_ptr(), libc::MFD_CLOEXEC) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error()).context("creating the bwrap argument file");
    }
    let mut file = File::from(unsafe { OwnedFd::from_raw_fd(raw) });
    for arg in args {
        file.write_all(arg.as_bytes())?;
        file.write_all(&[0])?;
    }
    file.rewind()?;
    Ok(file.into())
}

fn guest_path(raw: &str) -> anyhow::Result<LiteralPath> {
    LiteralPath::new(raw).with_context(|| format!("sandbox path `{raw}` is not a valid guest path"))
}

/// Mirrors the mounts `assemble` describes without a policy, with host paths
/// resolved so the planner never sees a symlink on the host side.
fn guest_layout(spec: &LaunchSpec) -> anyhow::Result<GuestLayout> {
    let mut layout = GuestLayout::new().with_working_dir(guest_path(&spec.workspace_sandbox_path.to_string_lossy())?);
    let canonical =
        |path: &Path| std::fs::canonicalize(path).with_context(|| format!("resolving bind source {}", path.display()));
    let access = |readonly: bool| {
        if readonly {
            MountAccess::ReadOnly
        } else {
            MountAccess::ReadWrite
        }
    };

    if Path::new("/usr").exists() {
        layout.bind(
            guest_path("/usr")?,
            canonical(Path::new("/usr"))?,
            MountAccess::ReadOnly,
        );
    }
    for dir in ROOT_SYSTEM_DIRS {
        let host = Path::new(dir);
        match std::fs::symlink_metadata(host) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = std::fs::read_link(host)?;
                layout.symlink(guest_path(dir)?, target.to_string_lossy().into_owned());
            }
            Ok(_) => {
                layout.bind(guest_path(dir)?, canonical(host)?, MountAccess::ReadOnly);
            }
            Err(_) => {}
        }
    }
    layout.bind(
        guest_path(&spec.workspace_sandbox_path.to_string_lossy())?,
        canonical(&spec.workspace_host_path)?,
        access(spec.workspace_readonly),
    );
    for bind in &spec.extra_binds {
        let dest = guest_path(&bind.sandbox_path)?;
        if bind.credential_mask {
            layout.mask_file(dest.clone());
        }
        layout.bind(dest, canonical(&bind.host_path)?, access(bind.readonly));
    }
    Ok(layout)
}

fn unrestricted_policy() -> anyhow::Result<FsPolicy> {
    Ok(FsPolicy::new(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![FsPathEntry::Literal {
            path: LiteralPath::new("/")?,
            directory: DirectoryMarker::default(),
        }],
        Vec::new(),
    )?)
}

fn layout_of(spec: &LaunchSpec, host_view: Option<&HostView>) -> anyhow::Result<GuestLayout> {
    match host_view {
        Some(view) => {
            let mut layout = host_layout()?.with_working_dir(guest_path(&view.cwd().to_string_lossy())?);
            for bind in &spec.extra_binds {
                let host = std::fs::canonicalize(&bind.host_path)
                    .with_context(|| format!("resolving bind source {}", bind.host_path.display()))?;
                let access = if bind.readonly {
                    MountAccess::ReadOnly
                } else {
                    MountAccess::ReadWrite
                };
                let dest = guest_path(&bind.sandbox_path)?;
                if bind.credential_mask {
                    layout.mask_file(dest.clone());
                }
                layout.bind(dest, host, access);
            }
            Ok(layout)
        }
        None => guest_layout(spec),
    }
}

fn proc_mount_args(spec: &LaunchSpec) -> Vec<String> {
    let flag = if spec.platform.linux.weaker_nested_sandbox.is_weaker() {
        "--bind"
    } else {
        "--proc"
    };
    [flag, "/proc"]
        .into_iter()
        .chain(spec.platform.linux.weaker_nested_sandbox.is_weaker().then_some("/proc"))
        .map(str::to_string)
        .collect()
}

/// Removes the `--new-session` flag the argument list carries.
fn drop_new_session(args: &mut Vec<String>) {
    if let Some(at) = args.iter().position(|a| a == "--new-session") {
        args.remove(at);
    }
}

#[async_trait]
impl IsolationBackend for BubblewrapBackend {
    async fn launch(&self, spec: &LaunchSpec) -> anyhow::Result<LaunchedProcess> {
        self.spawn(spec, None).await
    }

    async fn launch_terminal(&self, spec: &LaunchSpec, tty: &TerminalStdio) -> anyhow::Result<LaunchedProcess> {
        self.spawn(spec, Some(tty)).await
    }
}

impl BubblewrapBackend {
    async fn spawn(&self, spec: &LaunchSpec, tty: Option<&TerminalStdio>) -> anyhow::Result<LaunchedProcess> {
        super::ensure_platform_options_enforceable(&spec.platform, false, true)?;
        let relay = match &spec.egress_policy {
            EgressPolicy::Relay(egress) => Some(RelayWiring::open(egress)?),
            _ => None,
        };

        let fs_policy = match (&spec.fs_policy, &self.host_view) {
            (_, Some(_)) if spec.platform.filesystem.disabled.is_disabled() => Some(unrestricted_policy()?),
            (_, None) if spec.platform.filesystem.disabled.is_disabled() => {
                anyhow::bail!("turning the filesystem rules off needs the host view, which shows the host filesystem")
            }
            (Some(policy), _) => Some(policy.clone()),
            (None, Some(_)) => Some(FsPolicy::default()),
            (None, None) => None,
        };

        let init = self.guest_init.as_deref().map(InitWiring::open).transpose()?;
        let mut observer = None;
        if let Some(denials) = &spec.fs_denials {
            let writes = match &fs_policy {
                Some(policy) => {
                    let layout = layout_of(spec, self.host_view.as_ref())?;
                    let plan = plan_mounts_with(policy, &layout, &LinuxHostTree, &spec.platform.filesystem)?;
                    WriteMap::from_mounts(&plan)?
                }
                None => WriteMap::from_layout(spec)?,
            };
            observer = Some(ObserverWiring::start(
                &self.observer_binary()?,
                writes,
                denials.clone(),
            )?);
        }

        let guard = self.namespace_guard(spec);
        guard.warn_when_userns_open();
        let mut seccomp_readers = Vec::new();
        if relay.is_none() && observer.is_none() && !(init.is_some() && guard.uses_helper()) {
            for program in build_sandbox_filters(guard.seccomp())? {
                let (read, write) = nix::unistd::pipe()?;
                let mut writer = File::from(write);
                writer.write_all(&program)?;
                drop(writer);
                seccomp_readers.push(read);
            }
        }
        let seccomp_fds: Vec<i32> = seccomp_readers.iter().map(|fd| fd.as_raw_fd()).collect();

        let policy_mounts = match &fs_policy {
            Some(policy) => Some(PolicyMounts::prepare(
                spec,
                policy,
                relay.as_ref(),
                observer.as_ref(),
                init.as_ref(),
                &self.second_layer,
                self.host_view.as_ref(),
            )?),
            None => None,
        };
        let mut bwrap_args = Self::assemble(
            spec,
            &seccomp_fds,
            policy_mounts.as_ref().map(|m| m.args_fd.as_raw_fd()),
            relay.as_ref(),
            policy_mounts.as_ref().and_then(|m| m.confinement.as_ref()),
            observer.as_ref(),
            init.as_ref(),
            &guard,
        )?;
        if tty.is_some() {
            // `--new-session` detaches the payload from any terminal so it
            // cannot push input into the worker's. A terminal launch owns a
            // fresh pseudo-terminal that only this payload can reach, and it
            // needs that terminal as its controlling one. The seccomp filter
            // refuses TIOCSTI and TIOCLINUX, so the dropped flag costs no
            // input-injection protection.
            drop_new_session(&mut bwrap_args);
        }
        let mut cmd = Command::new(&self.bwrap_path);
        cmd.args(&bwrap_args);
        cmd.env_clear();
        for (key, value) in sandbox_env(spec, self.host_view.as_ref()) {
            cmd.env(key, value);
        }

        let limits = self.limits.prepare(&spec.limits)?;
        limits.install(&mut cmd);

        let mut inherited_fds = seccomp_fds.clone();
        if let Some(mounts) = &policy_mounts {
            inherited_fds.extend(mounts.inherited_fds());
        }
        if let Some(relay) = &relay {
            inherited_fds.extend(relay.inherited_fds());
        }
        if let Some(observer) = &observer {
            inherited_fds.extend(observer.inherited_fds());
        }
        if let Some(init) = &init {
            inherited_fds.push(init.inherited_fd());
        }
        unsafe {
            cmd.pre_exec(move || {
                for &fd in &inherited_fds {
                    if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }

        self.stdio.apply(&mut cmd);
        if let Some(tty) = tty {
            tty.apply(&mut cmd)?;
        }
        let child = cmd.spawn()?;
        drop(seccomp_readers);
        let placeholders = policy_mounts
            .as_ref()
            .map(|mounts| mounts.plan.placeholders.clone())
            .unwrap_or_default();
        let unguarded = policy_mounts
            .as_ref()
            .map(|mounts| mounts.plan.unguarded.clone())
            .unwrap_or_default();
        drop(policy_mounts);
        let pid = child.id().unwrap_or(0);
        let mut monitors = MonitorGuard::default();
        if let Some(guard) = observer.as_mut().and_then(ObserverWiring::take_guard) {
            monitors.hold(guard);
        }
        Ok(LaunchedProcess {
            pid,
            child: Arc::new(Mutex::new(Some(child))),
            limits: limits.into_guard(),
            placeholders,
            unguarded,
            monitors,
        })
    }
}

fn push_ro_bind(args: &mut Vec<String>, host: &Path, sandbox: &str) {
    BubblewrapBackend::push_ro_bind(args, host, sandbox)
}

/// The sandbox starts from an empty environment so worker secrets (NATS and
/// broker credentials) never leak in. Lease-provided values override the
/// defaults.
fn sandbox_env(spec: &LaunchSpec, host_view: Option<&HostView>) -> Vec<(String, String)> {
    if host_view.is_some() {
        return spec.env.clone();
    }
    let mut env = BTreeMap::from([
        ("PATH".to_string(), DEFAULT_PATH.to_string()),
        (
            "HOME".to_string(),
            spec.workspace_sandbox_path.to_string_lossy().into_owned(),
        ),
        ("TMPDIR".to_string(), "/tmp".to_string()),
    ]);
    env.extend(spec.env.iter().cloned());
    env.insert(RUNTIME_MARKER_VAR.to_string(), RUNTIME_MARKER_VALUE.to_string());
    env.into_iter().collect()
}

fn parse_uid_gid(user: &str) -> anyhow::Result<(u32, Option<u32>)> {
    match user.split_once(':') {
        Some((uid, gid)) => {
            let uid = uid
                .parse::<u32>()
                .map_err(|_| anyhow::anyhow!("bwrap requires a numeric uid, got `{uid}`"))?;
            let gid = gid
                .parse::<u32>()
                .map_err(|_| anyhow::anyhow!("bwrap requires a numeric gid, got `{gid}`"))?;
            Ok((uid, Some(gid)))
        }
        None => {
            let uid = user
                .parse::<u32>()
                .map_err(|_| anyhow::anyhow!("bwrap requires a numeric uid, got `{user}`"))?;
            Ok((uid, None))
        }
    }
}

#[cfg(test)]
mod tests;
