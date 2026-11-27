use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::num::NonZeroU32;
use std::os::unix::fs::{PermissionsExt, chown};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use trogon_sandbox_contract::LeaseId;

use crate::config::{FirecrackerConfig, KernelImage, MemoryMib, StateDir, VcpuCount};
use crate::error::ProviderError;
use crate::process::{VmLauncher, VmProcess};
use crate::vm_dir::VmDir;

#[derive(Debug, Clone)]
pub struct JailerConfig {
    pub binary: PathBuf,
    pub uid_base: NonZeroU32,
    pub gid_base: NonZeroU32,
    pub cgroup_parent: PathBuf,
    pub memory_overhead: MemoryMib,
}

pub struct JailerLauncher {
    config: JailerConfig,
    firecracker: PathBuf,
    executable: String,
    kernel: KernelImage,
    default_memory: MemoryMib,
    default_vcpus: VcpuCount,
    identity_count: u32,
    identities: Mutex<HashMap<PathBuf, u32>>,
}

impl JailerLauncher {
    pub fn new(config: JailerConfig, worker: &FirecrackerConfig) -> Result<Self, ProviderError> {
        if config.cgroup_parent.as_os_str().is_empty()
            || config
                .cgroup_parent
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(fail("jailer cgroup parent must be a relative path without traversal"));
        }
        for base in [config.uid_base, config.gid_base] {
            base.get()
                .checked_add(worker.max_concurrent_leases.saturating_sub(1))
                .ok_or_else(|| fail("jailer identity range overflows u32"))?;
        }
        if worker.max_concurrent_leases == 0 {
            return Err(fail("jailer requires a nonempty identity range"));
        }
        let firecracker = std::fs::canonicalize(worker.binary.as_path())
            .map_err(|error| fail(format!("resolving Firecracker binary: {error}")))?;
        let executable = firecracker
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| fail("Firecracker executable must have a UTF-8 filename"))?
            .to_owned();
        Ok(Self {
            config,
            firecracker,
            executable,
            kernel: worker.kernel.clone(),
            default_memory: worker.default_memory,
            default_vcpus: worker.default_vcpus,
            identity_count: worker.max_concurrent_leases,
            identities: Mutex::new(HashMap::new()),
        })
    }

    fn reserve_identity(&self, dir: &VmDir) -> Result<u32, ProviderError> {
        let mut identities = self
            .identities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if identities.contains_key(dir.as_path()) {
            return Err(fail("a jailer identity is already reserved for this lease"));
        }
        let held: BTreeSet<_> = identities.values().copied().collect();
        let offset = (0..self.identity_count)
            .find(|offset| !held.contains(offset))
            .ok_or_else(|| fail("no dedicated jailer identities are available"))?;
        identities.insert(dir.as_path().to_path_buf(), offset);
        Ok(offset)
    }

    fn cgroup_parent(&self, dir: &VmDir) -> Result<PathBuf, ProviderError> {
        let id = dir
            .as_path()
            .file_name()
            .ok_or_else(|| fail("lease directory has no name"))?;
        Ok(self.config.cgroup_parent.join(id))
    }

    fn arguments(
        &self,
        dir: &VmDir,
        offset: u32,
        memory: MemoryMib,
        vcpus: VcpuCount,
    ) -> Result<Vec<std::ffi::OsString>, ProviderError> {
        let memory_bytes = (u64::from(memory.get()) + u64::from(self.config.memory_overhead.get())) * 1024 * 1024;
        Ok([
            "--id".into(),
            "vm".into(),
            "--exec-file".into(),
            self.firecracker.as_os_str().to_owned(),
            "--uid".into(),
            (self.config.uid_base.get() + offset).to_string().into(),
            "--gid".into(),
            (self.config.gid_base.get() + offset).to_string().into(),
            "--chroot-base-dir".into(),
            dir.as_path().join("jail").into_os_string(),
            "--cgroup-version".into(),
            "2".into(),
            "--parent-cgroup".into(),
            self.cgroup_parent(dir)?.into_os_string(),
            "--cgroup".into(),
            format!("memory.max={memory_bytes}").into(),
            "--cgroup".into(),
            format!("cpu.max={} 100000", u32::from(vcpus.get()) * 100000).into(),
            "--cgroup".into(),
            format!("pids.max={}", u32::from(vcpus.get()) + 32).into(),
            "--resource-limit".into(),
            "no-file=1024".into(),
            "--".into(),
            "--api-sock".into(),
            "/api.sock".into(),
        ]
        .into())
    }

    async fn prepare(&self, dir: &VmDir, offset: u32) -> Result<(), ProviderError> {
        let kernel = dir.runtime_dir().join("vmlinux");
        tokio::fs::copy(self.kernel.as_path(), &kernel)
            .await
            .map_err(|error| fail(format!("copying guest kernel into jail: {error}")))?;
        for path in [
            dir.rootfs(),
            kernel,
            dir.runtime_dir().join(crate::snapshot::STATE_FILE),
            dir.runtime_dir().join(crate::snapshot::MEMORY_FILE),
        ] {
            if tokio::fs::try_exists(&path)
                .await
                .map_err(|error| fail(error.to_string()))?
            {
                chown(
                    &path,
                    Some(self.config.uid_base.get() + offset),
                    Some(self.config.gid_base.get() + offset),
                )
                .map_err(|error| fail(format!("setting jail resource ownership: {error}")))?;
                tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                    .await
                    .map_err(|error| fail(format!("setting jail resource permissions: {error}")))?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl VmLauncher for JailerLauncher {
    fn vm_dir(&self, state: &StateDir, lease: &LeaseId) -> Result<VmDir, ProviderError> {
        Ok(VmDir::jailed(state, lease, &self.executable)?)
    }

    fn kernel_path(&self, _configured: &KernelImage) -> KernelImage {
        KernelImage::new("/vmlinux")
    }

    async fn launch(&self, dir: &VmDir) -> Result<VmProcess, ProviderError> {
        self.launch_with_resources(dir, self.default_memory, self.default_vcpus)
            .await
    }

    async fn launch_with_resources(
        &self,
        dir: &VmDir,
        memory: MemoryMib,
        vcpus: VcpuCount,
    ) -> Result<VmProcess, ProviderError> {
        self.launch_with_network_resources(dir, memory, vcpus, None).await
    }

    async fn launch_with_network_resources(
        &self,
        dir: &VmDir,
        memory: MemoryMib,
        vcpus: VcpuCount,
        network_namespace: Option<&Path>,
    ) -> Result<VmProcess, ProviderError> {
        let offset = self.reserve_identity(dir)?;
        if let Some(namespace) = network_namespace {
            if !namespace.is_absolute() {
                return Err(fail("jailer network namespace must be an absolute path"));
            }
            crate::network::assign_tap_owner(
                namespace,
                self.config.uid_base.get() + offset,
                self.config.gid_base.get() + offset,
            )
            .await
            .map_err(|error| fail(format!("assigning jailed TAP ownership: {error}")))?;
        }
        self.prepare(dir, offset).await?;
        let log = File::create(dir.log_file()).map_err(|error| fail(error.to_string()))?;
        let stderr = log.try_clone().map_err(|error| fail(error.to_string()))?;
        let mut arguments = self.arguments(dir, offset, memory, vcpus)?;
        if let Some(namespace) = network_namespace {
            if !namespace.is_absolute() {
                return Err(fail("jailer network namespace must be an absolute path"));
            }
            let boundary = arguments
                .iter()
                .position(|arg| arg.as_os_str() == std::ffi::OsStr::new("--"))
                .ok_or_else(|| fail("jailer arguments lack a Firecracker boundary"))?;
            arguments.splice(boundary..boundary, ["--netns".into(), namespace.as_os_str().to_owned()]);
        }
        let child = Command::new(&self.config.binary)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| fail(format!("starting jailer: {error}")))?;
        let pid = child.id().ok_or_else(|| fail("jailer exited without a process id"))?;
        let mut process = VmProcess::from_child(child);
        if let Err(error) = tokio::fs::write(dir.pid_file(), pid.to_string()).await {
            process.stop().await;
            return Err(fail(format!("persisting jailer process id: {error}")));
        }
        Ok(process)
    }

    async fn cleanup(&self, dir: &VmDir) -> Result<(), ProviderError> {
        let parent = Path::new("/sys/fs/cgroup").join(self.cgroup_parent(dir)?);
        remove_cgroup_paths_with(
            [parent.join("vm"), parent],
            Duration::from_secs(5),
            tokio::fs::remove_dir,
        )
        .await?;
        self.identities
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(dir.as_path());
        Ok(())
    }
}

async fn remove_cgroup_paths_with<F, Fut>(
    paths: [PathBuf; 2],
    budget: Duration,
    mut remove: F,
) -> Result<(), ProviderError>
where
    F: FnMut(PathBuf) -> Fut + Send,
    Fut: std::future::Future<Output = std::io::Result<()>> + Send,
{
    let parent = paths[1].clone();
    tokio::time::timeout(budget, async {
        for path in paths {
            loop {
                match remove(path.clone()).await {
                    Ok(()) => break,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                    Err(error) if error.raw_os_error() == Some(libc::EBUSY) => {
                        wait_cgroup_empty(&path)
                            .await
                            .map_err(|error| fail(format!("checking cgroup {}: {error}", path.display())))?;
                        tokio::time::sleep(CGROUP_RETRY_INTERVAL).await;
                    }
                    Err(error) => {
                        return Err(fail(format!("removing cgroup {}: {error}", path.display())));
                    }
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| {
        fail(format!(
            "removing owned cgroups under {} exceeded {budget:?}",
            parent.display()
        ))
    })?
}

async fn wait_cgroup_empty(path: &Path) -> std::io::Result<()> {
    loop {
        match tokio::fs::read_to_string(path.join("cgroup.events")).await {
            Ok(events) if !cgroup_populated(&events)? => return Ok(()),
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !tokio::fs::try_exists(path).await? {
                    return Ok(());
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        }
        tokio::time::sleep(CGROUP_RETRY_INTERVAL).await;
    }
}

fn cgroup_populated(events: &str) -> std::io::Result<bool> {
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "cgroup.events needs one populated value of 0 or 1",
        )
    };
    let mut populated = None;
    for line in events.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() != Some("populated") {
            continue;
        }
        if populated.is_some() {
            return Err(invalid());
        }
        populated = Some(match (fields.next(), fields.next()) {
            (Some("0"), None) => false,
            (Some("1"), None) => true,
            _ => return Err(invalid()),
        });
    }
    populated.ok_or_else(invalid)
}

fn fail(message: impl Into<String>) -> ProviderError {
    ProviderError::Launch(message.into())
}

#[cfg(test)]
mod tests;

mod constants;
use constants::CGROUP_RETRY_INTERVAL;
