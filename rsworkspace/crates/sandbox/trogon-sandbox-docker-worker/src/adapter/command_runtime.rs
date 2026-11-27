use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, ensure};
use bollard::Docker;
use bollard::models::{
    ContainerCreateBody, HostConfig, Mount, MountPointTypeEnum, MountTypeEnum, MountVolumeOptions, VolumeCreateRequest,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, DownloadFromContainerOptionsBuilder, ListVolumesOptionsBuilder,
    RemoveContainerOptionsBuilder, RemoveVolumeOptionsBuilder, UploadToContainerOptionsBuilder,
};
use bytes::Bytes;
use futures::StreamExt;
use sha2::{Digest as _, Sha256};
use tokio::sync::OnceCell;

#[cfg(test)]
mod tests;

use super::constants::{DIGEST_LABEL, MAX_ARTIFACT_BYTES, NONCE_LABEL, OWNER_LABEL, RUNTIME_FILE_NAME};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Architecture {
    X86_64,
    Aarch64,
}

pub struct CommandRuntime {
    artifact: Arc<[u8]>,
    digest: String,
    architecture: Architecture,
    owner: String,
    nonce: String,
    volume_name: String,
    seed_name: String,
    failed: AtomicBool,
    volume: OnceCell<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeMount {
    volume: String,
    root: String,
    pub digest: String,
}

impl RuntimeMount {
    pub fn binding(&self) -> String {
        format!("{}:{}:ro,nocopy", self.volume, self.root)
    }
    pub fn executable(&self) -> String {
        format!("{}/{RUNTIME_FILE_NAME}", self.root)
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> anyhow::Result<u16> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .context("truncated ELF header")?
            .try_into()?,
    ))
}
fn u32_at(bytes: &[u8], offset: usize) -> anyhow::Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .context("truncated ELF header")?
            .try_into()?,
    ))
}

fn elf_architecture(bytes: &[u8]) -> anyhow::Result<Architecture> {
    ensure!(
        bytes.len() >= 64 && &bytes[..4] == b"\x7fELF" && bytes[4] == 2 && bytes[5] == 1 && bytes[6] == 1,
        "command supervisor requires a little-endian 64-bit Linux ELF"
    );
    ensure!(
        matches!(bytes[7], 0 | 3) && matches!(u16_at(bytes, 16)?, 2 | 3) && u32_at(bytes, 20)? == 1,
        "unsupported command supervisor ELF ABI"
    );
    let architecture = match u16_at(bytes, 18)? {
        62 => Architecture::X86_64,
        183 => Architecture::Aarch64,
        _ => anyhow::bail!("unsupported command supervisor architecture"),
    };
    let offset = usize::try_from(u64::from_le_bytes(bytes[32..40].try_into()?))?;
    let entry_size = usize::from(u16_at(bytes, 54)?);
    let entries = usize::from(u16_at(bytes, 56)?);
    ensure!(
        entry_size == 56 && (1..=512).contains(&entries),
        "invalid command supervisor program headers"
    );
    let end = offset
        .checked_add(entry_size.checked_mul(entries).context("ELF header overflow")?)
        .context("ELF header overflow")?;
    ensure!(end <= bytes.len(), "truncated command supervisor program headers");
    for index in 0..entries {
        ensure!(
            u32_at(bytes, offset + entry_size * index)? != 3,
            "command supervisor must be static and cannot load a guest interpreter"
        );
    }
    Ok(architecture)
}

fn labels(owner: &str, digest: &str, nonce: &str) -> HashMap<String, String> {
    HashMap::from([
        (OWNER_LABEL.into(), owner.into()),
        (DIGEST_LABEL.into(), digest.into()),
        (NONCE_LABEL.into(), nonce.into()),
    ])
}

async fn absent(docker: &Docker, container: &str) -> anyhow::Result<()> {
    ensure!(
        matches!(
            docker.inspect_container(container, None).await,
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
        ),
        "command runtime seed writer absence is unconfirmed"
    );
    Ok(())
}

impl CommandRuntime {
    pub fn load(path: &Path, owner: String) -> anyhow::Result<Self> {
        ensure!(
            !owner.is_empty() && owner.len() <= 1024 && !owner.chars().any(char::is_control),
            "invalid command runtime owner"
        );
        let mut file = std::fs::File::open(path).context("open configured command supervisor artifact")?;
        ensure!(
            file.metadata()?.is_file() && file.metadata()?.len() <= MAX_ARTIFACT_BYTES as u64,
            "command supervisor must be a bounded regular file"
        );
        let mut bytes = Vec::new();
        file.by_ref()
            .take((MAX_ARTIFACT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= MAX_ARTIFACT_BYTES,
            "command supervisor artifact exceeds its byte limit"
        );
        let architecture = elf_architecture(&bytes)?;
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let nonce = crate::fresh_id::fresh_id();
        let volume_name = format!("sandbox-command-{digest}-{nonce}");
        let seed_name = format!("sandbox-command-seed-{nonce}");
        Ok(Self {
            artifact: bytes.into(),
            digest,
            architecture,
            owner,
            volume_name,
            seed_name,
            nonce,
            failed: AtomicBool::new(false),
            volume: OnceCell::new(),
        })
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn volume_name(&self) -> &str {
        &self.volume_name
    }

    async fn remove_seed_writer(&self, docker: &Docker) -> anyhow::Result<()> {
        let inspected = match docker.inspect_container(&self.seed_name, None).await {
            Ok(inspected) => inspected,
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        ensure!(
            inspected
                .config
                .as_ref()
                .and_then(|config| config.labels.as_ref())
                .is_some_and(|actual| labels(&self.owner, &self.digest, &self.nonce)
                    .iter()
                    .all(|(key, value)| actual.get(key) == Some(value))),
            "command runtime seed writer ownership changed"
        );
        let writer = inspected.id.context("seed writer has no canonical identity")?;
        ensure!(
            writer.len() == 64 && writer.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "seed writer identity changed"
        );
        docker
            .remove_container(
                &writer,
                Some(RemoveContainerOptionsBuilder::new().force(true).v(true).build()),
            )
            .await?;
        absent(docker, &writer).await
    }

    async fn validate_volume(&self, docker: &Docker, name: &str) -> anyhow::Result<()> {
        let volume = docker.inspect_volume(name).await?;
        ensure!(
            volume.name == name
                && volume.driver == "local"
                && volume.options.is_empty()
                && volume.labels == labels(&self.owner, &self.digest, &self.nonce),
            "command runtime volume ownership or driver changed"
        );
        Ok(())
    }

    async fn verify_bytes(&self, docker: &Docker, container: &str, path: &str) -> anyhow::Result<()> {
        let mut stream = docker.download_from_container(
            container,
            Some(DownloadFromContainerOptionsBuilder::new().path(path).build()),
        );
        let mut archive = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            ensure!(
                archive
                    .len()
                    .checked_add(chunk.len())
                    .is_some_and(|length| length <= MAX_ARTIFACT_BYTES + 64 * 1024),
                "command runtime archive exceeds its byte limit"
            );
            archive.extend_from_slice(&chunk);
        }
        let mut archive = tar::Archive::new(Cursor::new(archive));
        let mut files = 0;
        for entry in archive.entries()? {
            let mut entry = entry?;
            ensure!(
                entry.header().entry_type().is_file() && entry.path()?.as_ref() == Path::new(RUNTIME_FILE_NAME),
                "command runtime archive has an unexpected entry"
            );
            ensure!(
                entry.size() == self.artifact.len() as u64
                    && entry.header().mode()? == 0o555
                    && entry.header().uid()? == 0
                    && entry.header().gid()? == 0,
                "command runtime artifact metadata changed"
            );
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            ensure!(
                bytes.as_slice() == self.artifact.as_ref(),
                "command runtime executable differs from its trusted artifact"
            );
            files += 1;
        }
        ensure!(files == 1, "command runtime archive lacks its unique executable");
        Ok(())
    }

    async fn seed(&self, docker: &Docker, image: &str) -> anyhow::Result<String> {
        let information = docker.info().await?;
        let actual = match information.architecture.as_deref() {
            Some("x86_64" | "amd64") => Architecture::X86_64,
            Some("aarch64" | "arm64") => Architecture::Aarch64,
            _ => anyhow::bail!("unsupported native Docker architecture"),
        };
        ensure!(
            actual == self.architecture,
            "command supervisor artifact does not match the Docker architecture"
        );
        let name = self.volume_name.clone();
        ensure!(
            matches!(
                docker.inspect_volume(&name).await,
                Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
            ),
            "command runtime volume already exists or cannot be inspected"
        );
        let created = docker
            .create_volume(VolumeCreateRequest {
                name: Some(name.clone()),
                driver: Some("local".into()),
                labels: Some(labels(&self.owner, &self.digest, &self.nonce)),
                ..Default::default()
            })
            .await?;
        ensure!(created.name == name, "Docker returned another runtime volume identity");
        let root = format!("/.trogon-sandbox-run-{}", crate::fresh_id::fresh_id());
        let result = async {
            self.validate_volume(docker, &name).await?;
            let writer = docker
                .create_container(
                    Some(CreateContainerOptionsBuilder::new().name(&self.seed_name).build()),
                    ContainerCreateBody {
                        image: Some(image.into()),
                        entrypoint: Some(vec![]),
                        cmd: Some(vec!["false".into()]),
                        labels: Some(labels(&self.owner, &self.digest, &self.nonce)),
                        host_config: Some(HostConfig {
                            mounts: Some(vec![Mount {
                                typ: Some(MountTypeEnum::VOLUME),
                                source: Some(name.clone()),
                                target: Some(root.clone()),
                                read_only: Some(false),
                                volume_options: Some(MountVolumeOptions {
                                    no_copy: Some(true),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }]),
                            network_mode: Some("none".into()),
                            cap_drop: Some(vec!["ALL".into()]),
                            security_opt: Some(vec!["no-new-privileges".into()]),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )
                .await?;
            ensure!(
                writer.id.len() == 64 && writer.id.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "seed writer identity is not canonical"
            );
            let inspected = docker.inspect_container(&writer.id, None).await?;
            ensure!(
                inspected.state.as_ref().and_then(|state| state.running) == Some(false)
                    && inspected
                        .mounts
                        .as_ref()
                        .is_some_and(
                            |mounts| mounts.iter().any(|mount| mount.typ == Some(MountPointTypeEnum::VOLUME)
                                && mount.name.as_deref() == Some(name.as_str())
                                && mount.destination.as_deref() == Some(root.as_str())
                                && mount.rw == Some(true))
                        ),
                "command runtime seed writer is not an owned stopped volume writer"
            );
            let mut archive = tar::Builder::new(Vec::new());
            let mut header = tar::Header::new_gnu();
            header.set_size(self.artifact.len() as u64);
            header.set_mode(0o555);
            header.set_uid(0);
            header.set_gid(0);
            header.set_cksum();
            archive.append_data(&mut header, RUNTIME_FILE_NAME, Cursor::new(self.artifact.as_ref()))?;
            let archive = archive.into_inner()?;
            docker
                .upload_to_container(
                    &writer.id,
                    Some(
                        UploadToContainerOptionsBuilder::new()
                            .path(&root)
                            .no_overwrite_dir_non_dir("true")
                            .build(),
                    ),
                    bollard::body_full(Bytes::from(archive)),
                )
                .await?;
            self.verify_bytes(docker, &writer.id, &format!("{root}/{RUNTIME_FILE_NAME}"))
                .await?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let cleanup = self.remove_seed_writer(docker).await;
        if result.is_err() || cleanup.is_err() {
            self.validate_volume(docker, &name).await?;
            docker
                .remove_volume(&name, Some(RemoveVolumeOptionsBuilder::new().force(false).build()))
                .await?;
            cleanup?;
            result?;
        }
        self.validate_volume(docker, &name).await?;
        Ok(name)
    }

    pub(crate) async fn mount(&self, docker: &Docker, seed_image: &str) -> anyhow::Result<RuntimeMount> {
        ensure!(
            !self.failed.load(Ordering::Acquire),
            "command runtime setup requires reconciliation after an uncertain attempt"
        );
        let docker = docker.clone().with_timeout(Duration::from_secs(15));
        let volume = self
            .volume
            .get_or_try_init(|| async {
                let result = self.seed(&docker, seed_image).await;
                if result.is_err() {
                    self.failed.store(true, Ordering::Release);
                }
                result
            })
            .await?;
        self.validate_volume(&docker, volume).await?;
        Ok(RuntimeMount {
            volume: volume.clone(),
            root: format!("/.trogon-sandbox-run-{}", crate::fresh_id::fresh_id()),
            digest: self.digest.clone(),
        })
    }

    pub(crate) async fn verify_mount(
        &self,
        docker: &Docker,
        container: &str,
        expected: &RuntimeMount,
    ) -> anyhow::Result<()> {
        self.validate_volume(docker, &expected.volume).await?;
        let inspected = docker.inspect_container(container, None).await?;
        ensure!(
            inspected.id.as_deref() == Some(container)
                && inspected.mounts.as_ref().is_some_and(|mounts| mounts
                    .iter()
                    .filter(|mount| mount.name.as_deref() == Some(expected.volume.as_str()))
                    .count()
                    == 1
                    && mounts.iter().any(|mount| mount.typ == Some(MountPointTypeEnum::VOLUME)
                        && mount.name.as_deref() == Some(expected.volume.as_str())
                        && mount.destination.as_deref() == Some(expected.root.as_str())
                        && mount.rw == Some(false))),
            "native container does not hold its exact immutable runtime volume"
        );
        self.verify_bytes(docker, container, &expected.executable()).await
    }

    pub(crate) async fn existing_mount(&self, docker: &Docker, container: &str) -> anyhow::Result<RuntimeMount> {
        let volume = self.volume.get().context("command runtime has not been seeded")?;
        let inspected = docker.inspect_container(container, None).await?;
        let root = inspected
            .mounts
            .as_ref()
            .and_then(|mounts| {
                mounts
                    .iter()
                    .find(|mount| mount.name.as_deref() == Some(volume.as_str()) && mount.rw == Some(false))
            })
            .and_then(|mount| mount.destination.clone())
            .context("container lacks its immutable command runtime mount")?;
        ensure!(
            root.strip_prefix("/.trogon-sandbox-run-")
                .is_some_and(crate::fresh_id::is_fresh_id),
            "command runtime mountpoint is not an unpredictable direct root mount"
        );
        let mount = RuntimeMount {
            volume: volume.clone(),
            root,
            digest: self.digest.clone(),
        };
        self.verify_mount(docker, container, &mount).await?;
        Ok(mount)
    }

    pub async fn shutdown(&self, docker: &Docker) -> anyhow::Result<()> {
        let docker = docker.clone().with_timeout(Duration::from_secs(15));
        self.remove_seed_writer(&docker).await?;
        match docker.inspect_volume(&self.volume_name).await {
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => return Ok(()),
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        let volume = &self.volume_name;
        {
            self.validate_volume(&docker, volume).await?;
            docker
                .remove_volume(volume, Some(RemoveVolumeOptionsBuilder::new().force(false).build()))
                .await?;
            ensure!(
                matches!(
                    docker.inspect_volume(volume).await,
                    Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
                ),
                "runtime volume absence remains unconfirmed"
            );
        }
        Ok(())
    }

    pub async fn sweep_owned(docker: &Docker, owner: &str) -> anyhow::Result<usize> {
        let filters = HashMap::from([(
            "label".to_string(),
            vec![
                format!("{OWNER_LABEL}={owner}"),
                DIGEST_LABEL.into(),
                NONCE_LABEL.into(),
            ],
        )]);
        let volumes = docker
            .list_volumes(Some(ListVolumesOptionsBuilder::new().filters(&filters).build()))
            .await?;
        let mut removed = 0;
        for volume in volumes.volumes.unwrap_or_default() {
            let Some(digest) = volume.labels.get(DIGEST_LABEL) else {
                continue;
            };
            let Some(nonce) = volume.labels.get(NONCE_LABEL) else {
                continue;
            };
            ensure!(
                volume.labels.get(OWNER_LABEL).is_some_and(|actual| actual == owner)
                    && digest.len() == 64
                    && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && crate::fresh_id::is_fresh_id(nonce)
                    && volume.name == format!("sandbox-command-{digest}-{nonce}")
                    && volume.driver == "local"
                    && volume.options.is_empty(),
                "runtime artifact sweep found ambiguous ownership"
            );
            docker
                .remove_volume(
                    &volume.name,
                    Some(RemoveVolumeOptionsBuilder::new().force(false).build()),
                )
                .await?;
            removed += 1;
        }
        Ok(removed)
    }
}
