mod sparse_copy;
pub use sparse_copy::copy_sparse;

use std::collections::HashMap;
use std::fs::File;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{Digest, OciImageRef};
use trogon_sandbox_oci::{OciClient, ResolvedContainerImage};

pub use crate::settings::OciRootfsConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedRootfs {
    pub identity: Digest,
    pub image: ResolvedContainerImage,
    pub path: PathBuf,
    pub rootfs_digest: Digest,
}

pub struct ImageCache {
    config: OciRootfsConfig,
    oci: Arc<OciClient>,
    conversion: Mutex<()>,
    process_lock: StdMutex<Option<(PathBuf, File)>>,
    verified: Mutex<HashMap<Digest, FileFingerprint>>,
}

impl Drop for ImageCache {
    fn drop(&mut self) {
        let held = self
            .process_lock
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, file)) = held
            && let Err(error) = fs2::FileExt::unlock(file)
        {
            tracing::warn!(%error, "releasing OCI image cache lock failed");
        }
    }
}

impl ImageCache {
    pub fn new(config: OciRootfsConfig, oci: Arc<OciClient>) -> Self {
        Self {
            config,
            oci,
            conversion: Mutex::new(()),
            process_lock: StdMutex::new(None),
            verified: Mutex::new(HashMap::new()),
        }
    }

    pub async fn resolve(&self, image: &OciImageRef) -> anyhow::Result<CachedRootfs> {
        tokio::time::timeout(Duration::from_secs(300), self.resolve_inner(image))
            .await
            .context("OCI image preparation exceeded 300 seconds")?
    }

    async fn resolve_inner(&self, image: &OciImageRef) -> anyhow::Result<CachedRootfs> {
        let agent = read_guest_binary(&self.config.guest_agent)
            .await
            .context("reading static guest agent")?;
        let init = read_guest_binary(&self.config.guest_init)
            .await
            .context("reading static guest init")?;
        validate_guest_binary(&agent).context("invalid guest agent executable")?;
        validate_guest_binary(&init).context("invalid guest init executable")?;
        let limits = trogon_sandbox_oci::ContainerPullLimits {
            max_blob_bytes: self.config.max_download_bytes,
            max_total_bytes: self.config.max_download_bytes,
        };
        let image = self.oci.resolve_container_image_with_limits(image, limits).await?;
        let identity = cache_identity(image.digest(), &agent, &init, self.config.disk_size_bytes)?;
        let _conversion = self.conversion.lock().await;
        let root = self.lock_root()?;
        let cache = root.join(identity.hex());
        if let Ok(stat) = std::fs::symlink_metadata(&cache) {
            ensure!(
                stat.is_dir() && !stat.file_type().is_symlink(),
                "cached image directory cannot be a symlink"
            );
        }
        if tokio::fs::try_exists(cache.join("complete.json")).await? {
            trusted_regular(&cache.join("complete.json"), false)?;
            let cached: CachedRootfs = serde_json::from_slice(&tokio::fs::read(cache.join("complete.json")).await?)?;
            ensure!(
                cached.identity == identity && cached.image == image && cached.path == cache.join("rootfs.ext4"),
                "cached image identity differs from its directory"
            );
            self.verify_cached(&cached).await?;
            return Ok(cached);
        }
        if tokio::fs::try_exists(&cache).await? {
            tokio::fs::remove_dir_all(&cache).await?;
        }
        let mut entries = tokio::fs::read_dir(&root).await?;
        let mut cached_bytes = 0u64;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                if entry.file_name().to_string_lossy().starts_with(".build-") {
                    tokio::fs::remove_dir_all(entry.path()).await?;
                    continue;
                }
                if let Ok(stat) = tokio::fs::metadata(entry.path().join("rootfs.ext4")).await {
                    cached_bytes = cached_bytes.saturating_add(stat.len());
                }
            }
        }
        ensure!(
            cached_bytes.saturating_add(self.config.disk_size_bytes) <= self.config.max_cache_bytes,
            "OCI rootfs cache byte budget is exhausted; remove unused cached bases or increase the budget"
        );
        let required = self
            .config
            .disk_size_bytes
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(image.compressed_bytes()))
            .and_then(|bytes| bytes.checked_add(1024 * 1024 * 1024))
            .context("OCI staging size overflows u64")?;
        ensure!(
            fs2::available_space(&root)? >= required,
            "insufficient free space for bounded OCI staging and root disk"
        );
        let build = tempfile::Builder::new().prefix(".build-").tempdir_in(&root)?;
        let layout = build.path().join("layout");
        self.oci
            .export_container_layout_with_limits(&image, &layout, limits)
            .await?;
        tokio::fs::write(build.path().join("agent"), agent).await?;
        tokio::fs::write(build.path().join("init"), init).await?;
        let script = build.path().join("build.sh");
        tokio::fs::write(&script, include_bytes!("../scripts/build-oci-rootfs.sh")).await?;
        build_in_namespace(&self.config, build.path(), &layout, &script).await?;
        let disk = build.path().join("rootfs.ext4");
        #[cfg(unix)]
        {
            tokio::fs::set_permissions(&disk, std::fs::Permissions::from_mode(0o444)).await?;
        }
        let (rootfs_digest, _) = file_digest(&disk).await?;
        let result = CachedRootfs {
            identity,
            image,
            path: cache.join("rootfs.ext4"),
            rootfs_digest,
        };
        tokio::fs::write(build.path().join("complete.json"), serde_json::to_vec(&result)?).await?;
        tokio::fs::remove_dir_all(layout).await?;
        tokio::fs::remove_file(script).await?;
        tokio::fs::rename(build.path(), &cache).await?;
        self.verified.lock().await.insert(
            result.identity.clone(),
            FileFingerprint::from(&trusted_regular(&result.path, true)?),
        );
        Ok(result)
    }

    fn lock_root(&self) -> anyhow::Result<PathBuf> {
        std::fs::create_dir_all(&self.config.cache_dir)?;
        let requested = std::fs::symlink_metadata(&self.config.cache_dir)?;
        ensure!(
            requested.is_dir() && !requested.file_type().is_symlink(),
            "OCI cache root cannot be a symlink"
        );
        let root = std::fs::canonicalize(&self.config.cache_dir)?;
        let uid = unsafe { libc::geteuid() };
        for ancestor in root.ancestors() {
            let stat = std::fs::metadata(ancestor)?;
            ensure!(
                stat.uid() == 0 || stat.uid() == uid,
                "OCI cache ancestors must belong to root or the worker user"
            );
            let mode = stat.permissions().mode();
            ensure!(
                mode & 0o022 == 0 || (stat.uid() == 0 && mode & 0o1000 != 0),
                "OCI cache ancestor is writable by other users"
            );
        }
        let mut held = self
            .process_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((locked, _)) = held.as_ref() {
            ensure!(*locked == root, "OCI cache root changed while the worker was running");
            return Ok(root);
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join(".worker.lock"))?;
        ensure!(file.metadata()?.is_file(), "OCI cache lock must be a regular file");
        fs2::FileExt::try_lock_exclusive(&file).context("OCI cache is already owned by another worker")?;
        *held = Some((root.clone(), file));
        Ok(root)
    }

    async fn verify_cached(&self, cached: &CachedRootfs) -> anyhow::Result<()> {
        let fingerprint = FileFingerprint::from(&trusted_regular(&cached.path, true)?);
        ensure!(
            fingerprint.len == self.config.disk_size_bytes,
            "cached rootfs size differs from configuration"
        );
        if self.verified.lock().await.get(&cached.identity) == Some(&fingerprint) {
            return Ok(());
        }
        let (digest, after) = file_digest(&cached.path).await?;
        ensure!(
            after == fingerprint && digest == cached.rootfs_digest,
            "cached rootfs content failed integrity verification"
        );
        self.verified.lock().await.insert(cached.identity.clone(), fingerprint);
        Ok(())
    }
}

pub fn cache_identity(image: &Digest, agent: &[u8], init: &[u8], disk_size: u64) -> anyhow::Result<Digest> {
    ensure!(disk_size >= 64 * 1024 * 1024, "OCI root disk must be at least 64 MiB");
    let mut hash = Sha256::new();
    hash.update(b"firecracker-ext4-v1\0");
    hash.update(image.as_str());
    hash.update(Sha256::digest(agent));
    hash.update(Sha256::digest(init));
    hash.update(disk_size.to_le_bytes());
    Ok(Digest::new(format!("sha256:{}", hex::encode(hash.finalize())))?)
}

pub fn validate_guest_binary(data: &[u8]) -> anyhow::Result<()> {
    let elf = goblin::elf::Elf::parse(data)?;
    ensure!(
        elf.is_64 && elf.little_endian && elf.header.e_machine == goblin::elf::header::EM_X86_64,
        "guest executable must be ELF64 little-endian amd64"
    );
    ensure!(
        elf.interpreter.is_none() && elf.libraries.is_empty(),
        "guest executable must be statically linked"
    );
    ensure!(
        elf.entry != 0
            && elf
                .program_headers
                .iter()
                .any(|header| header.p_type == goblin::elf::program_header::PT_LOAD),
        "guest executable has no loadable entry point"
    );
    Ok(())
}

async fn read_guest_binary(path: &Path) -> anyhow::Result<Vec<u8>> {
    ensure!(
        tokio::fs::metadata(path).await?.len() <= 64 * 1024 * 1024,
        "guest executable exceeds 64 MiB"
    );
    Ok(tokio::fs::read(path).await?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileFingerprint {
    device: u64,
    inode: u64,
    len: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

impl From<&std::fs::Metadata> for FileFingerprint {
    fn from(stat: &std::fs::Metadata) -> Self {
        Self {
            device: stat.dev(),
            inode: stat.ino(),
            len: stat.len(),
            mtime: stat.mtime(),
            mtime_nsec: stat.mtime_nsec(),
            ctime: stat.ctime(),
            ctime_nsec: stat.ctime_nsec(),
        }
    }
}

fn trusted_regular(path: &Path, readonly: bool) -> anyhow::Result<std::fs::Metadata> {
    let stat = std::fs::symlink_metadata(path)?;
    ensure!(
        stat.is_file() && !stat.file_type().is_symlink(),
        "OCI cache entry must be a regular file"
    );
    let uid = unsafe { libc::geteuid() };
    ensure!(
        stat.uid() == uid || stat.uid() == 0,
        "OCI cache entry has an unexpected owner"
    );
    ensure!(
        stat.permissions().mode() & if readonly { 0o222 } else { 0o022 } == 0,
        "OCI cache entry has unsafe write permissions"
    );
    Ok(stat)
}

async fn file_digest(path: &Path) -> anyhow::Result<(Digest, FileFingerprint)> {
    let mut file = tokio::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .await?;
    let before = FileFingerprint::from(&file.metadata().await?);
    let mut hash = Sha256::new();
    let mut buf = vec![0; 128 * 1024];
    loop {
        let read = file.read(&mut buf).await?;
        if read == 0 {
            break;
        }
        hash.update(&buf[..read]);
    }
    let after = FileFingerprint::from(&file.metadata().await?);
    ensure!(before == after, "OCI cache file changed during verification");
    Ok((Digest::new(format!("sha256:{}", hex::encode(hash.finalize())))?, after))
}

#[cfg(target_os = "linux")]
async fn build_in_namespace(
    config: &OciRootfsConfig,
    build: &Path,
    layout: &Path,
    script: &Path,
) -> anyhow::Result<()> {
    let parent = std::process::id();
    let mut command = tokio::process::Command::new("/usr/bin/unshare");
    command
        .args([
            "--mount",
            "--pid",
            "--fork",
            "--kill-child=KILL",
            "--mount-proc",
            "--propagation",
            "private",
            "/usr/bin/timeout",
            "-k",
            "5s",
            "160s",
            "/bin/sh",
        ])
        .arg(script)
        .arg(&config.umoci)
        .arg(&config.mkfs_ext4)
        .arg(layout)
        .arg(config.disk_size_bytes.to_string())
        .arg(build)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() as u32 != parent {
                return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let mut stderr = child.stderr.take().context("helper stderr is not piped")?;
    let mut drain = tokio::spawn(async move {
        let mut kept = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let read = stderr.read(&mut buf).await?;
            if read == 0 {
                break;
            }
            let available = (64 * 1024usize).saturating_sub(kept.len());
            kept.extend_from_slice(&buf[..read.min(available)]);
        }
        Ok::<_, std::io::Error>(kept)
    });
    let status = match tokio::time::timeout(Duration::from_secs(175), child.wait()).await {
        Ok(result) => result?,
        Err(_) => {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
            drain.abort();
            anyhow::bail!("bounded OCI extraction helper timed out");
        }
    };
    let diagnostics = match tokio::time::timeout(Duration::from_secs(5), &mut drain).await {
        Ok(result) => result??,
        Err(_) => {
            drain.abort();
            anyhow::bail!("OCI helper retained a stderr handle after exiting");
        }
    };
    ensure!(
        status.success(),
        "bounded OCI extraction failed: {}",
        String::from_utf8_lossy(&diagnostics)
    );
    Ok(())
}

#[cfg(not(target_os = "linux"))]
async fn build_in_namespace(
    _config: &OciRootfsConfig,
    _build: &Path,
    _layout: &Path,
    _script: &Path,
) -> anyhow::Result<()> {
    anyhow::bail!("OCI conversion requires Linux mount and PID namespaces")
}

#[cfg(test)]
mod tests;
