mod constants;
mod local_store;
pub use local_store::{Held, SnapshotFiles, SnapshotStore, SnapshotStoreError, Staged, StoredSnapshot};

use std::path::Path;

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::SnapshotReceipt;
use trogon_sandbox_contract::{ArtifactType, OciArtifactRef, SnapshotPushTarget};
use trogon_sandbox_oci::{Layer, OciClient, build_push_target};

use crate::api::FirecrackerApi;
use crate::config::{MemoryMib, VcpuCount};
use crate::environment::RestoreEnvironment;
pub use crate::snapshot_files::{FORMAT_VERSION, MEMORY_FILE, STATE_FILE};
use crate::vm_dir::VmDir;
use constants::{
    MAX_MEMORY_BYTES, MAX_METADATA_BYTES, MAX_ROOTFS_BYTES, MAX_STATE_BYTES, MAX_TRANSFER_BYTES, METADATA_FILE, ROLE,
    ROOTFS_FILE,
};
use std::io::Read;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use trogon_sandbox_contract::NetworkPolicy;

static CAPTURE_GATES: std::sync::OnceLock<crate::lease_gates::LeaseGates> = std::sync::OnceLock::new();

#[derive(Clone, Copy)]
pub struct RestoreMachine {
    pub memory: MemoryMib,
    pub vcpus: VcpuCount,
    pub rootfs_max_bytes: u64,
}

pub struct RestoredSnapshot {
    pub metadata: SnapshotMetadata,
    pub(crate) space: std::sync::Arc<crate::space::DiskReservation>,
}

struct BoundedBytes {
    data: Vec<u8>,
    remaining: u64,
    deadline: std::time::Instant,
}

impl std::io::Write for BoundedBytes {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if std::time::Instant::now() >= self.deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "snapshot compression deadline exceeded",
            ));
        }
        if data.len() as u64 > self.remaining {
            return Err(std::io::Error::new(
                std::io::ErrorKind::FileTooLarge,
                "snapshot compressed bytes exceed transfer budget",
            ));
        }
        self.data.extend_from_slice(data);
        self.remaining -= data.len() as u64;
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn restore_network_policy(
    saved: Option<&trogon_sandbox_contract::NetworkPolicy>,
    requested: Option<&trogon_sandbox_contract::NetworkPolicy>,
) -> Result<Option<trogon_sandbox_contract::NetworkPolicy>, crate::error::ProviderError> {
    match (saved, requested) {
        (Some(saved), requested) => Ok(Some(requested.unwrap_or(saved).clone())),
        (None, None | Some(NetworkPolicy::DenyAll)) => Ok(None),
        (None, Some(_)) => Err(crate::error::ProviderError::InvalidSpec(
            "a snapshot without a network device cannot gain one during restore".into(),
        )),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotMetadata {
    pub format_version: u32,
    pub architecture: String,
    pub memory_mib: u32,
    pub vcpus: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rootfs_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_defaults: Option<trogon_sandbox_oci::ResolvedContainerImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_policy: Option<trogon_sandbox_contract::NetworkPolicy>,
    /// Digest of the execution environment the snapshot was taken in. Absent in snapshots
    /// written before the field existed; see `LegacySnapshotPolicy` for how restore treats
    /// them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_digest: Option<trogon_sandbox_contract::EnvDigest>,
}

impl SnapshotMetadata {
    pub fn new(memory: MemoryMib, vcpus: VcpuCount) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            architecture: std::env::consts::ARCH.into(),
            memory_mib: memory.get(),
            vcpus: vcpus.get(),
            rootfs_bytes: None,
            image_defaults: None,
            network_policy: None,
            env_digest: None,
        }
    }

    pub fn with_env_digest(mut self, digest: Option<trogon_sandbox_contract::EnvDigest>) -> Self {
        self.env_digest = digest;
        self
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.format_version == FORMAT_VERSION,
            "unsupported Firecracker snapshot format"
        );
        ensure!(
            self.architecture == std::env::consts::ARCH,
            "snapshot architecture differs from this worker"
        );
        MemoryMib::new(self.memory_mib)?;
        ensure!(
            u64::from(self.memory_mib) * 1024 * 1024 <= MAX_MEMORY_BYTES,
            "snapshot memory exceeds 16 GiB"
        );
        VcpuCount::new(self.vcpus)?;
        if let Some(bytes) = self.rootfs_bytes {
            ensure!(
                bytes > 0 && bytes <= MAX_ROOTFS_BYTES,
                "snapshot root filesystem exceeds 4 GiB or is empty"
            );
        }
        Ok(())
    }
}

/// Manifest annotations for a snapshot: the environment digest, so the snapshot builder can
/// bind it into the signature and a registry reader can see it without unpacking layers.
pub fn manifest_annotations(metadata: &SnapshotMetadata) -> trogon_sandbox_oci::Annotations {
    let mut annotations = trogon_sandbox_oci::Annotations::new();
    if let Some(digest) = &metadata.env_digest {
        annotations.insert(
            trogon_sandbox_contract::EnvDigest::ANNOTATION_KEY.to_string(),
            digest.to_string(),
        );
    }
    annotations
}

pub struct LocalSnapshot {
    dir: tempfile::TempDir,
    pub metadata: SnapshotMetadata,
    space: std::sync::Arc<crate::space::DiskReservation>,
}

struct PauseRecovery {
    vm: VmDir,
    gate: Option<tokio::sync::OwnedMutexGuard<()>>,
    space: std::sync::Arc<crate::space::DiskReservation>,
    armed: bool,
    generation: Option<CaptureGeneration>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct InodeIdentity {
    device: u64,
    inode: u64,
}

impl InodeIdentity {
    fn of(metadata: &std::fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
    fn at(path: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::other("snapshot source path became a symlink"));
        }
        Ok(Self::of(&metadata))
    }
}

struct CaptureGeneration {
    lease: std::fs::File,
    runtime: std::fs::File,
    socket: InodeIdentity,
    pid_record: Option<(std::fs::File, String, Option<u64>)>,
}

impl CaptureGeneration {
    fn capture(vm: &VmDir) -> anyhow::Result<Self> {
        let open = |path: &Path| {
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)
        };
        let lease = open(vm.as_path())?;
        let runtime = open(vm.runtime_dir())?;
        ensure!(
            lease.metadata()?.is_dir() && runtime.metadata()?.is_dir(),
            "snapshot source is not a directory"
        );
        let socket = InodeIdentity::at(&vm.api_socket())?;
        let pid_record = match open(&vm.pid_file()) {
            Ok(mut file) => {
                ensure!(
                    file.metadata()?.is_file() && file.metadata()?.len() <= 128,
                    "snapshot PID record is invalid"
                );
                let mut body = String::new();
                (&mut file).take(129).read_to_string(&mut body)?;
                ensure!(body.len() <= 128, "snapshot PID record exceeds its bound");
                let pid: u32 = body.trim().parse()?;
                Some((file, body, process_start(pid)?))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let generation = Self {
            lease,
            runtime,
            socket,
            pid_record,
        };
        ensure!(
            generation.matches(vm)?,
            "snapshot source changed during capture admission"
        );
        Ok(generation)
    }

    fn matches(&self, vm: &VmDir) -> anyhow::Result<bool> {
        if InodeIdentity::at(vm.as_path())? != InodeIdentity::of(&self.lease.metadata()?)
            || InodeIdentity::at(vm.runtime_dir())? != InodeIdentity::of(&self.runtime.metadata()?)
            || InodeIdentity::at(&vm.api_socket())? != self.socket
        {
            return Ok(false);
        }
        match &self.pid_record {
            None => Ok(
                matches!(std::fs::symlink_metadata(vm.pid_file()), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
            ),
            Some((file, body, start)) => {
                if InodeIdentity::at(&vm.pid_file())? != InodeIdentity::of(&file.metadata()?) {
                    return Ok(false);
                }
                let mut actual = String::new();
                std::fs::File::open(vm.pid_file())?
                    .take(129)
                    .read_to_string(&mut actual)?;
                if actual != *body {
                    return Ok(false);
                }
                let pid: u32 = body.trim().parse()?;
                Ok(process_start(pid)? == *start)
            }
        }
    }
}

fn process_start(pid: u32) -> anyhow::Result<Option<u64>> {
    #[cfg(target_os = "linux")]
    {
        let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let tail = stat.rsplit_once(") ").context("process stat is malformed")?.1;
        return Ok(Some(
            tail.split_whitespace()
                .nth(19)
                .context("process start time is missing")?
                .parse()?,
        ));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        Ok(None)
    }
}

impl Drop for PauseRecovery {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let vm = self.vm.clone();
        let gate = self.gate.take();
        let space = std::sync::Arc::clone(&self.space);
        let Some(generation) = self.generation.take() else {
            return;
        };
        let cleanup = async move {
            let _gate = gate;
            let _space = space;
            if !generation.matches(&vm).unwrap_or(false) {
                tracing::warn!("cancelled snapshot source generation changed; refusing recovery");
                return;
            }
            let resumed = tokio::time::timeout(
                std::time::Duration::from_secs(20),
                FirecrackerApi::new(vm.api_socket()).resume(),
            )
            .await;
            if !matches!(resumed, Ok(Ok(()))) {
                if !generation.matches(&vm).unwrap_or(false) {
                    return;
                }
                tracing::warn!("cancelled snapshot could not resume its source; stopping the owned VMM");
                match tokio::time::timeout(std::time::Duration::from_secs(20), crate::process::reap_orphan(&vm)).await {
                    Ok(Ok(())) => {}
                    result => tracing::error!(?result, "cancelled snapshot source cleanup failed"),
                }
            }
        };
        if let Err(error) = std::thread::Builder::new()
            .name("snapshot-resume".into())
            .spawn(
                move || match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(runtime) => runtime.block_on(cleanup),
                    Err(error) => {
                        tracing::error!(%error, "snapshot cleanup runtime could not start")
                    }
                },
            )
        {
            tracing::error!(%error, "snapshot cleanup thread could not start");
        }
    }
}

impl LocalSnapshot {
    #[cfg(test)]
    pub(crate) fn fixture(parent: &Path, memory_mib: u32) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir_in(parent)?;
        std::fs::write(dir.path().join(MEMORY_FILE), vec![0; memory_mib as usize * 1024 * 1024])?;
        std::fs::write(dir.path().join(ROOTFS_FILE), b"rootfs")?;
        std::fs::write(dir.path().join(STATE_FILE), b"state")?;
        let mut metadata = SnapshotMetadata::new(MemoryMib::new(memory_mib)?, VcpuCount::new(2)?);
        metadata.rootfs_bytes = Some(6);
        std::fs::write(dir.path().join(METADATA_FILE), serde_json::to_vec(&metadata)?)?;
        let space = crate::space::reserve(parent, 1024 * 1024 * u64::from(memory_mib) + 6)?;
        Ok(Self { dir, metadata, space })
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub(crate) fn space(&self) -> std::sync::Arc<crate::space::DiskReservation> {
        std::sync::Arc::clone(&self.space)
    }

    pub async fn capture(vm: &VmDir, mut metadata: SnapshotMetadata) -> anyhow::Result<Self> {
        metadata.rootfs_bytes = Some(tokio::fs::metadata(vm.rootfs()).await?.len());
        metadata.validate()?;
        let bytes = metadata
            .rootfs_bytes
            .context("snapshot capture has no rootfs size")?
            .checked_add(u64::from(metadata.memory_mib) * 1024 * 1024)
            .and_then(|bytes| bytes.checked_add(MAX_STATE_BYTES + MAX_METADATA_BYTES))
            .context("snapshot capture reservation overflows")?;
        let space = crate::space::reserve(vm.runtime_dir(), bytes)?;
        let identity = trogon_sandbox_contract::LeaseId::new(
            std::fs::canonicalize(vm.api_socket())?.to_string_lossy().into_owned(),
        );
        let gate = CAPTURE_GATES.get_or_init(Default::default).lock(&identity).await;
        let generation = CaptureGeneration::capture(vm)?;
        let capture = tempfile::Builder::new()
            .prefix("capture-")
            .tempdir_in(vm.runtime_dir())?;
        #[cfg(unix)]
        {
            let owner = std::fs::metadata(vm.runtime_dir())?;
            std::os::unix::fs::chown(capture.path(), Some(owner.uid()), Some(owner.gid()))?;
        }
        let relative = capture.path().strip_prefix(vm.runtime_dir())?;
        let api = FirecrackerApi::new(vm.api_socket());
        let mut recovery = PauseRecovery {
            vm: vm.clone(),
            gate: Some(gate),
            space: std::sync::Arc::clone(&space),
            armed: true,
            generation: Some(generation),
        };
        api.pause().await?;
        let result = async {
            api.create_snapshot_files(relative.join(STATE_FILE), relative.join(MEMORY_FILE))
                .await?;
            crate::disk::copy_private_reserved(
                &vm.rootfs(),
                &capture.path().join(ROOTFS_FILE),
                std::sync::Arc::clone(&space),
            )
            .await?;
            tokio::fs::write(capture.path().join(METADATA_FILE), serde_json::to_vec(&metadata)?).await?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        ensure!(
            recovery
                .generation
                .as_ref()
                .context("capture generation is missing")?
                .matches(vm)?,
            "snapshot source generation changed during capture"
        );
        let resumed = api.resume().await;
        if resumed.is_ok() {
            recovery.armed = false;
        }
        resumed.context("resuming VM after snapshot capture")?;
        result?;
        let dir = tempfile::Builder::new()
            .prefix("snapshot-")
            .tempdir_in(vm.as_path().parent().context("lease directory has no parent")?)?;
        tokio::fs::remove_dir(dir.path()).await?;
        tokio::fs::rename(capture.path(), dir.path()).await?;
        Ok(Self { dir, metadata, space })
    }

    pub async fn layers(&self) -> anyhow::Result<Vec<Layer>> {
        layers_from_directory(self.path()).await
    }

    pub async fn seal_to(self, destination: &Path) -> anyhow::Result<u64> {
        let info = directory_info(self.path()).await?;
        for file in [METADATA_FILE, STATE_FILE, MEMORY_FILE, ROOTFS_FILE] {
            let path = self.path().join(file);
            std::os::unix::fs::chown(
                &path,
                Some(unsafe { libc::geteuid() }),
                Some(unsafe { libc::getegid() }),
            )?;
            tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).await?;
            tokio::fs::File::open(path).await?.sync_all().await?;
        }
        std::os::unix::fs::chown(
            self.path(),
            Some(unsafe { libc::geteuid() }),
            Some(unsafe { libc::getegid() }),
        )?;
        tokio::fs::set_permissions(self.path(), std::fs::Permissions::from_mode(0o700)).await?;
        std::fs::File::open(self.path())?.sync_all()?;
        tokio::fs::rename(self.path(), destination).await?;
        std::fs::File::open(destination.parent().context("snapshot destination has no parent")?)?.sync_all()?;
        Ok(info.bytes)
    }

    pub async fn push(&self, oci: &OciClient, target: &SnapshotPushTarget) -> anyhow::Result<SnapshotReceipt> {
        push_directory(oci, target, self.path()).await
    }
}

pub struct SnapshotDirectoryInfo {
    pub metadata: SnapshotMetadata,
    pub bytes: u64,
}

pub async fn directory_info(path: &Path) -> anyhow::Result<SnapshotDirectoryInfo> {
    let mut count = 0;
    let mut entries = tokio::fs::read_dir(path).await?;
    let mut bytes = 0u64;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        ensure!(
            [METADATA_FILE, STATE_FILE, MEMORY_FILE, ROOTFS_FILE]
                .iter()
                .any(|file| std::ffi::OsStr::new(file) == name),
            "unexpected file in retained snapshot"
        );
        let stat = tokio::fs::symlink_metadata(entry.path()).await?;
        ensure!(
            stat.is_file() && !stat.file_type().is_symlink(),
            "snapshot files must be regular files"
        );
        bytes = bytes.checked_add(stat.len()).context("snapshot size overflows u64")?;
        if name == METADATA_FILE {
            ensure!(stat.len() <= MAX_METADATA_BYTES, "snapshot metadata exceeds 2 MiB");
        }
        if name == STATE_FILE {
            ensure!(stat.len() <= MAX_STATE_BYTES, "snapshot state exceeds 16 MiB");
        }
        if name == ROOTFS_FILE {
            ensure!(stat.len() <= MAX_ROOTFS_BYTES, "snapshot root filesystem exceeds 4 GiB");
        }
        count += 1;
    }
    ensure!(count == 4, "retained snapshot is incomplete");
    let metadata: SnapshotMetadata = serde_json::from_slice(&tokio::fs::read(path.join(METADATA_FILE)).await?)?;
    metadata.validate()?;
    if let Some(expected) = metadata.rootfs_bytes {
        ensure!(
            tokio::fs::metadata(path.join(ROOTFS_FILE)).await?.len() == expected,
            "retained root filesystem size differs from metadata"
        );
    }
    ensure!(
        tokio::fs::metadata(path.join(MEMORY_FILE)).await?.len() == u64::from(metadata.memory_mib) * 1024 * 1024,
        "retained memory size differs from metadata"
    );
    Ok(SnapshotDirectoryInfo { metadata, bytes })
}

pub async fn layers_from_directory(directory: &Path) -> anyhow::Result<Vec<Layer>> {
    layers_with_budget(directory, MAX_TRANSFER_BYTES).await
}

async fn layers_with_budget(directory: &Path, mut budget: u64) -> anyhow::Result<Vec<Layer>> {
    directory_info(directory).await?;
    let mut layers = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    for file in [METADATA_FILE, STATE_FILE, MEMORY_FILE, ROOTFS_FILE] {
        let path = directory.join(file);
        let compressed = matches!(file, MEMORY_FILE | ROOTFS_FILE);
        let data = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<u8>> {
            if compressed {
                let mut encoder = flate2::write::GzEncoder::new(
                    BoundedBytes {
                        data: Vec::new(),
                        remaining: budget,
                        deadline,
                    },
                    flate2::Compression::fast(),
                );
                std::io::copy(&mut std::fs::File::open(path)?, &mut encoder)?;
                Ok(encoder.finish()?.data)
            } else {
                ensure!(
                    std::fs::metadata(&path)?.len() <= budget,
                    "snapshot exceeds transfer budget"
                );
                Ok(std::fs::read(path)?)
            }
        })
        .await??;
        budget = budget
            .checked_sub(data.len() as u64)
            .context("snapshot exceeds transfer budget")?;
        layers.push(Layer {
            data,
            media_type: format!(
                "application/vnd.trogon.firecracker.{file}.v1{}",
                if compressed { "+gzip" } else { "" }
            ),
            annotations: [(ROLE.to_string(), file.to_string())].into_iter().collect(),
        });
    }
    Ok(layers)
}

pub async fn push_directory(
    oci: &OciClient,
    target: &SnapshotPushTarget,
    directory: &Path,
) -> anyhow::Result<SnapshotReceipt> {
    ensure!(
        target.artifact_type == ArtifactType::FirecrackerSnapshotV1,
        "snapshot target must use FirecrackerSnapshotV1"
    );
    let layers = layers_from_directory(directory).await?;
    let bytes = layers.iter().map(|layer| layer.data.len() as u64).sum();
    let annotations = manifest_annotations(&directory_info(directory).await?.metadata);
    let push_target = build_push_target(
        &target.registry,
        &target.repository,
        ArtifactType::FirecrackerSnapshotV1,
        &layers,
        &annotations,
    )?;
    oci.push_artifact(&push_target, layers, annotations).await?;
    Ok(SnapshotReceipt {
        digest: push_target.digest,
        bytes,
        artifact_type: ArtifactType::FirecrackerSnapshotV1,
    })
}
pub async fn pull_into(
    oci: &OciClient,
    reference: &OciArtifactRef,
    vm: &VmDir,
    machine: RestoreMachine,
    config: &crate::config::FirecrackerConfig,
) -> anyhow::Result<RestoredSnapshot> {
    ensure!(
        reference.artifact_type == ArtifactType::FirecrackerSnapshotV1,
        "restore requires FirecrackerSnapshotV1"
    );
    let artifact = oci
        .pull_artifact_bounded(
            reference,
            trogon_sandbox_oci::ArtifactPullLimits {
                max_layers: 4,
                max_layer_bytes: MAX_TRANSFER_BYTES,
                max_total_bytes: MAX_TRANSFER_BYTES,
                timeout: std::time::Duration::from_secs(180),
            },
        )
        .await?;
    let manifest_label = artifact
        .annotations
        .get(trogon_sandbox_contract::EnvDigest::ANNOTATION_KEY)
        .map(|label| label.parse())
        .transpose()
        .context("snapshot manifest carries a malformed environment digest")?;
    let signed_label = manifest_label.clone();
    let restored = restore_layers_validated(
        &artifact.layers,
        vm.runtime_dir(),
        Some(machine),
        Some(&RestoreEnvironment {
            policy: &config.environment,
            manifest_label,
        }),
    )
    .await?;
    // The digest the snapshot records (metadata is checked against the manifest label above)
    // must also be what the signature vouches for.
    config
        .restore_signature
        .check(
            reference,
            restored.metadata.env_digest.as_ref().or(signed_label.as_ref()),
        )
        .await?;
    Ok(restored)
}

pub async fn restore_layers(layers: &[Layer], destination: &Path) -> anyhow::Result<SnapshotMetadata> {
    Ok(restore_layers_validated(layers, destination, None, None)
        .await?
        .metadata)
}

async fn restore_layers_validated(
    layers: &[Layer],
    destination: &Path,
    machine: Option<RestoreMachine>,
    environment: Option<&RestoreEnvironment<'_>>,
) -> anyhow::Result<RestoredSnapshot> {
    let expected = [METADATA_FILE, STATE_FILE, MEMORY_FILE, ROOTFS_FILE];
    ensure!(
        layers.len() == expected.len(),
        "Firecracker snapshot must contain metadata, VM state, memory and rootfs"
    );
    let mut files = std::collections::BTreeMap::new();
    let mut transfer = 0u64;
    for layer in layers {
        transfer = transfer
            .checked_add(layer.data.len() as u64)
            .context("snapshot bytes overflow")?;
        ensure!(transfer <= MAX_TRANSFER_BYTES, "snapshot exceeds transfer byte budget");
        let role = layer.annotations.get(ROLE).context("snapshot layer has no file role")?;
        ensure!(
            expected.contains(&role.as_str()),
            "unknown Firecracker snapshot file role"
        );
        ensure!(
            files.insert(role.as_str(), layer).is_none(),
            "duplicate Firecracker snapshot file role"
        );
    }
    ensure!(
        files[METADATA_FILE].data.len() as u64 <= MAX_METADATA_BYTES,
        "snapshot metadata exceeds 2 MiB"
    );
    ensure!(
        files[STATE_FILE].data.len() as u64 <= MAX_STATE_BYTES,
        "snapshot state exceeds 16 MiB"
    );
    let metadata: SnapshotMetadata = serde_json::from_slice(&files[METADATA_FILE].data)?;
    metadata.validate()?;
    if let Some(environment) = environment {
        if let (Some(recorded), Some(label)) = (&metadata.env_digest, &environment.manifest_label) {
            ensure!(
                recorded == label,
                "snapshot metadata and manifest disagree on the environment digest"
            );
        }
        let recorded = metadata.env_digest.as_ref().or(environment.manifest_label.as_ref());
        crate::environment::check_restore(environment.policy, recorded)?;
    }
    if let Some(machine) = machine {
        ensure!(
            metadata.memory_mib == machine.memory.get() && metadata.vcpus == machine.vcpus.get(),
            "snapshot memory and vCPU configuration must match the lease and worker"
        );
    }
    let rootfs_limit = machine
        .map(|machine| machine.rootfs_max_bytes)
        .unwrap_or(MAX_ROOTFS_BYTES)
        .min(MAX_ROOTFS_BYTES);
    let rootfs_bytes = metadata.rootfs_bytes.unwrap_or(rootfs_limit);
    ensure!(
        rootfs_bytes > 0 && rootfs_bytes <= rootfs_limit,
        "snapshot root filesystem exceeds the worker disk budget"
    );
    let required = (u64::from(metadata.memory_mib) * 1024 * 1024)
        .checked_add(rootfs_bytes)
        .and_then(|bytes| bytes.checked_add(MAX_STATE_BYTES + MAX_METADATA_BYTES))
        .context("snapshot disk reservation overflows")?;
    let space = crate::space::reserve(destination, required)?;
    let stage = tempfile::Builder::new().prefix("restore-").tempdir_in(destination)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    for file in expected {
        let data = files[file].data.clone();
        let compressed = files[file].media_type.ends_with("+gzip");
        let path = stage.path().join(file);
        let limit = if file == MEMORY_FILE {
            u64::from(metadata.memory_mib) * 1024 * 1024
        } else if file == ROOTFS_FILE {
            rootfs_bytes
        } else if file == STATE_FILE {
            MAX_STATE_BYTES
        } else {
            MAX_METADATA_BYTES
        };
        let writing_space = std::sync::Arc::clone(&space);
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut reader: Box<dyn Read + '_> = if compressed {
                Box::new(flate2::read::MultiGzDecoder::new(data.as_slice()))
            } else {
                Box::new(data.as_slice())
            };
            let exact = file == MEMORY_FILE || (file == ROOTFS_FILE && metadata.rootfs_bytes.is_some());
            materialize_file(&mut *reader, &path, limit, exact, deadline, writing_space)
        })
        .await??;
    }
    for file in expected {
        tokio::fs::rename(stage.path().join(file), destination.join(file)).await?;
    }
    Ok(RestoredSnapshot { metadata, space })
}

fn materialize_file(
    reader: &mut dyn std::io::Read,
    path: &Path,
    limit: u64,
    exact: bool,
    deadline: std::time::Instant,
    _reservation: std::sync::Arc<crate::space::DiskReservation>,
) -> anyhow::Result<()> {
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    let mut written = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        ensure!(
            std::time::Instant::now() < deadline,
            "snapshot decompression deadline exceeded"
        );
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        written += read as u64;
        ensure!(written <= limit, "snapshot file exceeds its size limit");
        if buffer[..read].iter().all(|byte| *byte == 0) {
            output.seek(SeekFrom::Current(read as i64))?;
        } else {
            output.write_all(&buffer[..read])?;
        }
    }
    output.set_len(written)?;
    ensure!(!exact || written == limit, "snapshot file size differs from metadata");
    Ok(())
}

#[cfg(test)]
mod tests;
