use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::process::Child;
use tokio::sync::Mutex;
use trogon_sandbox_contract::LiteralPath;

use super::limits::LimitGuard;
#[cfg(target_os = "linux")]
use crate::launch::EgressPolicy;
use crate::launch::LaunchSpec;

/// What a child's standard stream is connected to.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum StdioMode {
    #[default]
    Inherit,
    Piped,
    Null,
}

impl StdioMode {
    fn stdio(self) -> std::process::Stdio {
        match self {
            Self::Inherit => std::process::Stdio::inherit(),
            Self::Piped => std::process::Stdio::piped(),
            Self::Null => std::process::Stdio::null(),
        }
    }
}

/// How a backend connects the payload's standard streams. A worker leaves
/// them inherited; an embedding process captures output.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct ChildStdio {
    pub stdin: StdioMode,
    pub stdout: StdioMode,
    pub stderr: StdioMode,
}

impl ChildStdio {
    /// No input, output and errors captured through pipes.
    pub fn captured() -> Self {
        Self {
            stdin: StdioMode::Null,
            stdout: StdioMode::Piped,
            stderr: StdioMode::Piped,
        }
    }

    pub fn apply(&self, cmd: &mut tokio::process::Command) {
        cmd.stdin(self.stdin.stdio())
            .stdout(self.stdout.stdio())
            .stderr(self.stderr.stdio());
    }
}

pub struct LaunchedProcess {
    pub pid: u32,
    pub child: Arc<Mutex<Option<Child>>>,
    pub limits: LimitGuard,
    /// Empty files bubblewrap created on the host as mount points for
    /// guarded paths. They are removed once the process exits.
    pub placeholders: Vec<PathBuf>,
    pub unguarded: Vec<LiteralPath>,
    /// Whatever reads violations back for this launch; dropped once the
    /// process has exited.
    pub monitors: MonitorGuard,
}

/// Keeps the monitors of a launch alive for as long as it is held.
#[derive(Default)]
pub struct MonitorGuard(Vec<Box<dyn std::any::Any + Send>>);

impl MonitorGuard {
    pub fn hold(&mut self, monitor: impl std::any::Any + Send) {
        self.0.push(Box::new(monitor));
    }
}

/// The slave side of a pseudo-terminal a launch is attached to. It becomes
/// the payload's standard streams and its controlling terminal, so line
/// editing, job control and Ctrl-C behave as they do in a real terminal.
pub struct TerminalStdio {
    slave: std::os::fd::OwnedFd,
}

impl TerminalStdio {
    pub fn new(slave: std::os::fd::OwnedFd) -> Self {
        Self { slave }
    }

    /// Wires the slave to all three streams and makes the child a session
    /// leader whose controlling terminal is the slave.
    pub fn apply(&self, cmd: &mut tokio::process::Command) -> std::io::Result<()> {
        cmd.stdin(std::process::Stdio::from(self.slave.try_clone()?))
            .stdout(std::process::Stdio::from(self.slave.try_clone()?))
            .stderr(std::process::Stdio::from(self.slave.try_clone()?));
        // SAFETY: setsid and ioctl are async-signal-safe and touch only the
        // child, after the standard streams were already redirected.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }
}

#[async_trait]
pub trait IsolationBackend: Send + Sync {
    async fn launch(&self, spec: &LaunchSpec) -> anyhow::Result<LaunchedProcess>;

    /// Launches `spec` under the same confinement as [`Self::launch`], with
    /// `tty` as its terminal. A backend that cannot do that refuses.
    async fn launch_terminal(&self, spec: &LaunchSpec, tty: &TerminalStdio) -> anyhow::Result<LaunchedProcess> {
        let _ = (spec, tty);
        anyhow::bail!("this isolation backend cannot attach a terminal")
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn share_network(egress: &EgressPolicy) -> bool {
    match egress {
        EgressPolicy::DenyAll | EgressPolicy::Relay(_) => false,
        EgressPolicy::AllowAll => true,
    }
}
