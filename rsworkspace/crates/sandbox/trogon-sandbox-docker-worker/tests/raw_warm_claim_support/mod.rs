use crate::targeted_process_support::fresh_id;
use std::collections::HashMap;
use std::io::Read;
use std::time::Duration;

use anyhow::{Context, ensure};
use bollard::Docker;
use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, DownloadFromContainerOptionsBuilder, RemoveContainerOptionsBuilder,
};
use futures::StreamExt;

const OWNER: &str = "sandbox.trogon.live/owner";
const LEASE: &str = "sandbox.trogon.live/lease-id";
const ROLE: &str = "sandbox.trogon.live/role";
pub const OWN_MARKER: &str = "/tmp/raw-warm-native-owner";
pub const FORBIDDEN_MARKER: &str = "/tmp/raw-warm-untrusted-command";

pub struct KnownNative {
    pub id: String,
    owner: String,
    lease: String,
    image: String,
    init_pid: i64,
    started_at: String,
    marker: String,
}

impl KnownNative {
    pub async fn create(docker: &Docker, owner: &str, lease: &str, image: &str) -> anyhow::Result<Self> {
        let marker = fresh_id();
        let created = docker
            .create_container(
                Some(CreateContainerOptionsBuilder::new().name(lease).build()),
                ContainerCreateBody {
                    image: Some(image.into()),
                    entrypoint: Some(vec!["node".into()]),
                    cmd: Some(vec![
                        "-e".into(),
                        format!("require('fs').writeFileSync('{OWN_MARKER}','{marker}');setInterval(()=>{{}},1000)"),
                    ]),
                    labels: Some(HashMap::from([
                        (OWNER.into(), owner.into()),
                        (LEASE.into(), lease.into()),
                        (ROLE.into(), "workload".into()),
                    ])),
                    host_config: Some(HostConfig {
                        network_mode: Some("none".into()),
                        memory: Some(256 * 1024 * 1024),
                        nano_cpus: Some(1_000_000_000),
                        pids_limit: Some(64),
                        cap_drop: Some(vec!["ALL".into()]),
                        security_opt: Some(vec!["no-new-privileges:true".into()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        let initial = async {
            docker.start_container(&created.id, None).await?;
            let inspected = docker.inspect_container(&created.id, None).await?;
            let state = inspected.state.context("known fixture native state absent")?;
            ensure!(
                inspected.id.as_ref() == Some(&created.id)
                    && inspected.image.as_deref() == Some(image)
                    && state.running == Some(true),
                "fixture creation did not return its exact running native"
            );
            let native = Self {
                id: created.id.clone(),
                owner: owner.into(),
                lease: lease.into(),
                image: image.into(),
                init_pid: state
                    .pid
                    .filter(|pid| *pid > 0)
                    .context("known fixture init PID absent")?,
                started_at: state.started_at.context("known fixture StartedAt absent")?.to_string(),
                marker,
            };
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    match read_file(docker, &native.id, OWN_MARKER).await? {
                        Some(value) if value == native.marker.as_bytes() => {
                            return Ok::<_, anyhow::Error>(());
                        }
                        Some(_) => anyhow::bail!("known native original marker changed"),
                        None => tokio::time::sleep(Duration::from_millis(20)).await,
                    }
                }
            })
            .await
            .context("known fixture original marker readiness deadline")??;
            native.verify(docker).await?;
            Ok::<_, anyhow::Error>(native)
        }
        .await;
        if initial.is_err() {
            let inspected = docker.inspect_container(&created.id, None).await?;
            ensure!(
                inspected.id.as_ref() == Some(&created.id)
                    && inspected
                        .config
                        .as_ref()
                        .and_then(|config| config.labels.as_ref())
                        .is_some_and(|labels| labels.get(OWNER).is_some_and(|value| value == owner)
                            && labels.get(LEASE).is_some_and(|value| value == lease)),
                "failed native setup refuses changed fixture ownership"
            );
            docker
                .remove_container(
                    &created.id,
                    Some(RemoveContainerOptionsBuilder::new().force(true).v(true).build()),
                )
                .await?;
        }
        initial
    }

    pub async fn verify(&self, docker: &Docker) -> anyhow::Result<()> {
        let inspected = docker.inspect_container(&self.id, None).await?;
        let state = inspected.state.context("known fixture current native state absent")?;
        ensure!(
            inspected.id.as_ref() == Some(&self.id)
                && inspected.image.as_ref() == Some(&self.image)
                && state.running == Some(true)
                && state.pid == Some(self.init_pid)
                && state.started_at.as_ref().map(ToString::to_string).as_deref() == Some(self.started_at.as_str())
                && inspected
                    .config
                    .as_ref()
                    .and_then(|config| config.labels.as_ref())
                    .is_some_and(|labels| labels.get(OWNER) == Some(&self.owner)
                        && labels.get(LEASE) == Some(&self.lease)
                        && labels.get(ROLE).is_some_and(|role| role == "workload")),
            "known native original birth custody changed"
        );
        ensure!(
            read_file(docker, &self.id, OWN_MARKER).await?.as_deref() == Some(self.marker.as_bytes()),
            "known native original marker was modified or removed"
        );
        Ok(())
    }

    pub async fn forbidden_effect_absent(&self, docker: &Docker) -> anyhow::Result<bool> {
        Ok(read_file(docker, &self.id, FORBIDDEN_MARKER).await?.is_none())
    }

    pub async fn finish(&self, docker: &Docker) -> anyhow::Result<()> {
        match docker.inspect_container(&self.id, None).await {
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => return Ok(()),
            Err(error) => return Err(error.into()),
            Ok(inspected) => ensure!(
                inspected.id.as_ref() == Some(&self.id)
                    && inspected
                        .config
                        .as_ref()
                        .and_then(|config| config.labels.as_ref())
                        .is_some_and(
                            |labels| labels.get(OWNER) == Some(&self.owner) && labels.get(LEASE) == Some(&self.lease)
                        ),
                "planned fixture teardown refuses changed native ownership"
            ),
        }
        docker
            .remove_container(
                &self.id,
                Some(RemoveContainerOptionsBuilder::new().force(true).v(true).build()),
            )
            .await?;
        ensure!(
            matches!(
                docker.inspect_container(&self.id, None).await,
                Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
            ),
            "exact planned fixture native404 unconfirmed"
        );
        Ok(())
    }
}

async fn read_file(docker: &Docker, container: &str, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
    let mut stream = docker.download_from_container(
        container,
        Some(DownloadFromContainerOptionsBuilder::new().path(path).build()),
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => return Ok(None),
            Err(error) => return Err(error.into()),
            Ok(chunk) => chunk,
        };
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= 16 * 1024,
            "known fixture file archive exceeds bound"
        );
        bytes.extend_from_slice(&chunk);
    }
    let mut archive = tar::Archive::new(bytes.as_slice());
    let mut entries = archive.entries()?;
    let mut entry = entries.next().context("known fixture file archive is empty")??;
    ensure!(
        entry.header().entry_type().is_file() && entry.size() <= 1024,
        "known fixture file metadata changed"
    );
    let mut content = Vec::new();
    entry.read_to_end(&mut content)?;
    drop(entry);
    ensure!(entries.next().is_none(), "known fixture file archive has extra entries");
    Ok(Some(content))
}
