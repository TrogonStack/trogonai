//! Starting and stopping the VMM process.
//!
//! Launching sits behind [`VmLauncher`] so a jailer-based launch can replace the direct one
//! without touching the lease flow. The direct launcher runs `firecracker` as the worker's own
//! user, which is the MVP and not a hardened deployment.

use std::fs::File;
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tracing::warn;

use crate::config::{FirecrackerBinary, KernelImage, MemoryMib, StateDir, VcpuCount};
use crate::error::ProviderError;
use crate::vm_dir::VmDir;

#[async_trait]
pub trait VmLauncher: Send + Sync {
    fn vm_dir(&self, state: &StateDir, lease: &trogon_sandbox_contract::LeaseId) -> Result<VmDir, ProviderError> {
        Ok(VmDir::for_lease(state, lease)?)
    }

    fn kernel_path(&self, configured: &KernelImage) -> KernelImage {
        configured.clone()
    }

    async fn cleanup(&self, _dir: &VmDir) -> Result<(), ProviderError> {
        Ok(())
    }

    async fn launch_with_resources(
        &self,
        dir: &VmDir,
        _memory: MemoryMib,
        _vcpus: VcpuCount,
    ) -> Result<VmProcess, ProviderError> {
        self.launch(dir).await
    }

    async fn launch_with_network_resources(
        &self,
        dir: &VmDir,
        memory: MemoryMib,
        vcpus: VcpuCount,
        network_namespace: Option<&Path>,
    ) -> Result<VmProcess, ProviderError> {
        if network_namespace.is_some() {
            return Err(ProviderError::Launch(
                "this launcher cannot enter a network namespace".into(),
            ));
        }
        self.launch_with_resources(dir, memory, vcpus).await
    }

    async fn launch_with_network(
        &self,
        dir: &VmDir,
        memory: MemoryMib,
        vcpus: VcpuCount,
        network: Option<&crate::network::LeaseNetwork>,
    ) -> Result<VmProcess, ProviderError> {
        if network.is_some_and(|network| network.legacy().is_some()) {
            return Err(ProviderError::Launch(
                "this launcher cannot use the legacy network helper; use --host-links for jailer networking".into(),
            ));
        }
        let namespace = network.map(|network| network.namespace_path());
        self.launch_with_network_resources(dir, memory, vcpus, namespace.as_deref())
            .await
    }

    /// Starts a VMM whose API socket will appear at `dir.api_socket()`.
    async fn launch(&self, dir: &VmDir) -> Result<VmProcess, ProviderError>;
}

/// Runs the `firecracker` binary directly, with its output captured in the lease directory.
pub struct DirectLauncher {
    binary: FirecrackerBinary,
}

impl DirectLauncher {
    pub fn new(binary: FirecrackerBinary) -> Self {
        Self { binary }
    }

    async fn launch_in_namespace(&self, dir: &VmDir, namespace: Option<&Path>) -> Result<VmProcess, ProviderError> {
        let log = File::create(dir.log_file())
            .map_err(|e| ProviderError::Launch(format!("creating {}: {e}", dir.log_file().display())))?;
        let log_err = log
            .try_clone()
            .map_err(|e| ProviderError::Launch(format!("duplicating the log handle: {e}")))?;

        let binary = self.binary.as_path();
        let binary = if binary.components().count() > 1 {
            std::path::absolute(binary).map_err(|e| ProviderError::Launch(format!("resolving {binary:?}: {e}")))?
        } else {
            binary.to_path_buf()
        };
        let socket = std::path::absolute(dir.api_socket())
            .map_err(|e| ProviderError::Launch(format!("resolving API socket: {e}")))?;
        let mut command = if let Some(namespace) = namespace {
            if ![Path::new("/var/run/netns"), Path::new("/run/netns")]
                .iter()
                .any(|root| namespace.parent() == Some(*root))
            {
                return Err(ProviderError::Launch(
                    "network namespace must be a named namespace under /run/netns".into(),
                ));
            }
            let name = namespace
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| {
                    name.starts_with(|c: char| c.is_ascii_alphanumeric())
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                })
                .ok_or_else(|| ProviderError::Launch("invalid network namespace name".into()))?;
            let mut command = Command::new("ip");
            command.args(["netns", "exec", name]).arg(binary);
            command
        } else {
            Command::new(binary)
        };
        let child = command
            .arg("--api-sock")
            .arg(socket)
            .current_dir(dir.runtime_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            // A dropped handle must not leave a VM running with nobody to stop it.
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| ProviderError::Launch(format!("{}: {e}", self.binary)))?;

        if let Some(pid) = child.id() {
            // Best effort: only used to reap a VMM a crashed worker left behind.
            let _ = tokio::fs::write(dir.pid_file(), pid.to_string()).await;
        }
        Ok(VmProcess::from_child(child))
    }
}

#[async_trait]
impl VmLauncher for DirectLauncher {
    async fn launch_with_network(
        &self,
        dir: &VmDir,
        memory: MemoryMib,
        vcpus: VcpuCount,
        network: Option<&crate::network::LeaseNetwork>,
    ) -> Result<VmProcess, ProviderError> {
        if let Some(legacy) = network.and_then(|network| network.legacy()) {
            let log = File::create(dir.log_file()).map_err(|error| ProviderError::Launch(error.to_string()))?;
            let stderr = log
                .try_clone()
                .map_err(|error| ProviderError::Launch(error.to_string()))?;
            let binary = self.binary.clone();
            let child = legacy
                .vmm_command(&binary, dir)
                .current_dir(dir.runtime_dir())
                .stdin(Stdio::null())
                .stdout(Stdio::from(log))
                .stderr(Stdio::from(stderr))
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| ProviderError::Launch(error.to_string()))?;
            return Ok(VmProcess::supervise(
                child,
                Signal::RecordedPid {
                    pid_file: dir.pid_file(),
                    api_socket: dir.api_socket(),
                },
            ));
        }
        let namespace = network.map(|network| network.namespace_path());
        self.launch_with_network_resources(dir, memory, vcpus, namespace.as_deref())
            .await
    }

    async fn launch(&self, dir: &VmDir) -> Result<VmProcess, ProviderError> {
        self.launch_in_namespace(dir, None).await
    }

    async fn launch_with_network_resources(
        &self,
        dir: &VmDir,
        _memory: MemoryMib,
        _vcpus: VcpuCount,
        network_namespace: Option<&Path>,
    ) -> Result<VmProcess, ProviderError> {
        self.launch_in_namespace(dir, network_namespace).await
    }
}

/// A running VMM. Stopping it more than once is harmless.
/// Resolves once the VMM has exited, with a description of how it ended.
#[derive(Clone)]
pub struct VmExit(watch::Receiver<Option<String>>);

impl VmExit {
    pub fn status(&self) -> Option<String> {
        self.0.borrow().clone()
    }

    pub async fn wait(mut self) -> String {
        match self.0.wait_for(Option::is_some).await {
            Ok(status) => status.clone().unwrap_or_default(),
            Err(_) => "the VMM supervisor ended without a status".to_string(),
        }
    }
}

/// A running VMM, owned by a task that reaps it the moment it exits, so a VM that dies on
/// its own is noticed without anyone polling. Stopping it more than once is harmless.
pub struct VmProcess {
    pid: Option<u32>,
    exit: VmExit,
    stop: Option<oneshot::Sender<()>>,
    supervisor: Option<JoinHandle<()>>,
}

/// How the VMM is stopped.
pub enum Signal {
    /// The child is the VMM itself.
    Child,
    /// The child is `sudo`, which the worker cannot signal, so the relay below it records
    /// its own pid and that process is killed instead.
    RecordedPid { pid_file: PathBuf, api_socket: PathBuf },
}

impl Signal {
    async fn stop(&self, child: &mut Child) -> String {
        match self {
            Signal::Child => {
                if let Err(e) = child.start_kill() {
                    warn!(error = %e, "stopping firecracker failed");
                }
                describe(child.wait().await)
            }
            Signal::RecordedPid { pid_file, api_socket } => {
                if let Some(lease_dir) = pid_file.parent()
                    && let (Some(state), Some(name)) = (lease_dir.parent(), lease_dir.file_name())
                    && let Ok(dir) = VmDir::for_lease(
                        &StateDir::new(state),
                        &trogon_sandbox_contract::LeaseId::new(name.to_string_lossy().into_owned()),
                    )
                    && dir.api_socket() == *api_socket
                    && let Err(error) = reap_orphan(&dir).await
                {
                    warn!(%error,"stopping owned relay failed");
                }
                match tokio::time::timeout(EXIT_GRACE, child.wait()).await {
                    Ok(status) => describe(status),
                    Err(_) => {
                        warn!("the namespaced firecracker did not exit after its relay was killed");
                        "still running after its relay was killed".to_string()
                    }
                }
            }
        }
    }
}

fn describe(status: std::io::Result<std::process::ExitStatus>) -> String {
    match status {
        Ok(status) => status.to_string(),
        Err(e) => format!("waiting on firecracker failed: {e}"),
    }
}

impl VmProcess {
    pub fn from_child(child: Child) -> Self {
        Self::supervise(child, Signal::Child)
    }
    pub fn has_exited(&mut self) -> bool {
        self.exit.status().is_some()
    }

    pub fn supervise(mut child: Child, signal: Signal) -> Self {
        let pid = child.id();
        let (exit_tx, exit_rx) = watch::channel(None);
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let supervisor = tokio::spawn(async move {
            let described = tokio::select! {
                status = child.wait() => describe(status),
                // A dropped sender also lands here, so losing the handle still kills the VM.
                _ = stop_rx => signal.stop(&mut child).await,
            };
            let _ = exit_tx.send(Some(described));
        });
        Self {
            pid,
            exit: VmExit(exit_rx),
            stop: Some(stop_tx),
            supervisor: Some(supervisor),
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn exit(&self) -> VmExit {
        self.exit.clone()
    }

    /// Waits for the API socket to accept connections, failing early if the VMM dies first.
    ///
    /// The socket file appears at bind, before Firecracker listens on it, so a VMM under load
    /// can leave a window where the path exists but a connect is refused.
    pub async fn wait_for_api_socket(&mut self, socket: &Path, timeout: Duration) -> Result<(), ProviderError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if socket.exists() && UnixStream::connect(socket).await.is_ok() {
                return Ok(());
            }
            if let Some(status) = self.exit.status() {
                return Err(ProviderError::VmmExited(status));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(ProviderError::ApiSocketTimeout {
                    seconds: timeout.as_secs(),
                });
            }
            tokio::time::sleep(SOCKET_POLL_INTERVAL).await;
        }
    }

    pub async fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.await;
        }
    }
}

pub async fn reap_orphan(dir: &VmDir) -> Result<(), ProviderError> {
    #[cfg(target_os = "linux")]
    {
        for pid in owned_processes(dir).await? {
            if process_is_owned(dir, pid).await? {
                stop_orphan(pid).await?;
            }
        }
        if !owned_processes(dir).await?.is_empty() {
            return Err(ProviderError::Launch(
                "a lease-owned process survived stale recovery".into(),
            ));
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = dir;
    Ok(())
}

#[cfg(target_os = "linux")]
async fn owned_processes(dir: &VmDir) -> Result<Vec<u32>, ProviderError> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut entries = tokio::fs::read_dir("/proc")
            .await
            .map_err(|error| ProviderError::Launch(format!("scanning processes for stale VM: {error}")))?;
        let mut owned = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| ProviderError::Launch(format!("scanning stale VM processes: {error}")))?
        {
            let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
                continue;
            };
            if process_is_owned(dir, pid).await? {
                owned.push(pid);
            }
        }
        Ok(owned)
    })
    .await
    .map_err(|_| ProviderError::Launch("stale VM process scan exceeded 5s".into()))?
}

#[cfg(target_os = "linux")]
async fn process_is_owned(dir: &VmDir, pid: u32) -> Result<bool, ProviderError> {
    let cmdline = match tokio::fs::read(format!("/proc/{pid}/cmdline")).await {
        Ok(cmdline) => cmdline,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(ProviderError::Launch(format!(
                "checking stale VM process {pid}: {error}"
            )));
        }
    };
    let socket = std::path::absolute(dir.api_socket())
        .map_err(|error| ProviderError::Launch(format!("resolving stale VM socket: {error}")))?;
    let arguments: Vec<_> = cmdline.split(|byte| *byte == 0).collect();
    let direct = arguments
        .windows(2)
        .any(|pair| pair[0] == b"--api-sock" && pair[1] == socket.as_os_str().as_encoded_bytes());
    let jailed = arguments
        .windows(2)
        .any(|pair| pair == [b"--api-sock".as_slice(), b"/api.sock".as_slice()])
        && jailed_root_matches(dir, pid).await;
    let jail_base = dir.as_path().join("jail");
    let preparing_jail = arguments
        .windows(2)
        .any(|pair| pair[0] == b"--chroot-base-dir" && pair[1] == jail_base.as_os_str().as_encoded_bytes())
        && arguments
            .windows(2)
            .any(|pair| pair == [b"--id".as_slice(), b"vm".as_slice()]);
    Ok(direct || jailed || preparing_jail)
}

#[cfg(target_os = "linux")]
async fn stop_orphan(pid: u32) -> Result<(), ProviderError> {
    let mut command = Command::new("kill");
    command.arg("-KILL").arg(pid.to_string()).kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(2), async {
        let status = command
            .status()
            .await
            .map_err(|error| ProviderError::Launch(format!("reaping stale VM {pid}: {error}")))?;
        if !status.success() && tokio::fs::try_exists(format!("/proc/{pid}")).await.unwrap_or(true) {
            return Err(ProviderError::Launch(format!(
                "reaping stale VM {pid}: kill returned {status}"
            )));
        }
        loop {
            let Ok(stat) = tokio::fs::read_to_string(format!("/proc/{pid}/stat")).await else {
                return Ok(());
            };
            if stat
                .rsplit_once(") ")
                .is_some_and(|(_, tail)| tail.starts_with('Z') || tail.starts_with('X'))
            {
                return Ok(());
            }
            tokio::time::sleep(SOCKET_POLL_INTERVAL).await;
        }
    })
    .await
    .map_err(|_| ProviderError::Launch(format!("stale VM {pid} did not exit within 2s")))?
}

#[cfg(target_os = "linux")]
async fn jailed_root_matches(dir: &VmDir, pid: u32) -> bool {
    let Ok(actual) = tokio::fs::metadata(format!("/proc/{pid}/root")).await else {
        return false;
    };
    let Ok(mut entries) = tokio::fs::read_dir(dir.as_path().join("jail")).await else {
        return false;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if !entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let Ok(expected) = tokio::fs::metadata(entry.path().join("vm/root")).await else {
            continue;
        };
        if (actual.dev(), actual.ino()) == (expected.dev(), expected.ino()) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod main_supervision_tests;

mod constants;
use constants::{EXIT_GRACE, SOCKET_POLL_INTERVAL};
