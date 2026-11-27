//! Removing the runtime seed writer retires the anonymous volumes its image declared and keeps the named runtime volume.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, ensure};
use bollard::Docker;
use bollard::models::{
    ContainerConfig, ContainerCreateBody, HostConfig, Mount, MountPointTypeEnum, MountTypeEnum, MountVolumeOptions,
    VolumeCreateRequest,
};
use bollard::query_parameters::{
    CommitContainerOptionsBuilder, CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder,
    RemoveImageOptionsBuilder, RemoveVolumeOptionsBuilder,
};
use trogon_std::env::{ReadEnv, SystemEnv};

use super::super::{CommandRuntime, labels};
use crate::fresh_id::fresh_id;

const BASE_IMAGE: &str = "docker.io/library/node:22-bookworm-slim";
const IMAGE_VOLUME: &str = "/seed-writer-fixture-anonymous-volume";

fn missing<T>(result: &Result<T, bollard::errors::Error>) -> bool {
    matches!(
        result,
        Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
    )
}

fn canonical_container(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn canonical_image(id: &str) -> bool {
    id.strip_prefix("sha256:").is_some_and(canonical_container)
}

struct Fixture {
    docker: Docker,
    runtime: CommandRuntime,
    source_name: String,
    source_id: Option<String>,
    writer_id: Option<String>,
    image_reference: String,
    image_id: Option<String>,
    base_id: String,
    anonymous: Vec<String>,
}

struct Observation {
    removal_succeeded: bool,
    writer_absent: bool,
    anonymous_absent: bool,
    named_survived: bool,
}

impl Fixture {
    async fn new() -> anyhow::Result<Self> {
        let artifact = ReadEnv::var_os(&SystemEnv, "SANDBOX_DOCKER_COMMAND_SUPERVISOR")
            .context("seed teardown fixture requires SANDBOX_DOCKER_COMMAND_SUPERVISOR")?;
        let nonce = fresh_id();
        let runtime = CommandRuntime::load(Path::new(&artifact), format!("runtime-seed-{nonce}"))?;
        let docker = Docker::connect_with_defaults()?.with_timeout(Duration::from_secs(15));
        let base = docker
            .inspect_image(BASE_IMAGE)
            .await
            .context("seed teardown fixture requires the explicitly cached Node image")?;
        let base_id = base.id.context("cached base image lacks its canonical identity")?;
        ensure!(canonical_image(&base_id), "base image identity is not canonical");
        ensure!(
            base.config
                .and_then(|config| config.volumes)
                .is_none_or(|volumes| volumes.is_empty()),
            "cached fixture base must not already declare image volumes"
        );
        let mut fixture = Self {
            docker,
            runtime,
            source_name: format!("seed-writer-fixture-source-{nonce}"),
            source_id: None,
            writer_id: None,
            image_reference: format!("seed-writer-fixture-volume:{nonce}"),
            image_id: None,
            base_id,
            anonymous: Vec::new(),
        };
        if let Err(error) = fixture.prepare().await {
            fixture
                .cleanup()
                .await
                .context("finish exact fixture teardown after setup failure")?;
            return Err(error);
        }
        Ok(fixture)
    }

    fn owned_labels(&self) -> HashMap<String, String> {
        labels(&self.runtime.owner, &self.runtime.digest, &self.runtime.nonce)
    }

    async fn prepare(&mut self) -> anyhow::Result<()> {
        ensure!(
            missing(&self.docker.inspect_container(&self.source_name, None).await),
            "fixture source name already exists or cannot be inspected"
        );
        let source = self
            .docker
            .create_container(
                Some(CreateContainerOptionsBuilder::new().name(&self.source_name).build()),
                ContainerCreateBody {
                    image: Some(self.base_id.clone()),
                    entrypoint: Some(Vec::new()),
                    cmd: Some(vec!["false".into()]),
                    labels: Some(self.owned_labels()),
                    host_config: Some(HostConfig {
                        network_mode: Some("none".into()),
                        cap_drop: Some(vec!["ALL".into()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        ensure!(
            canonical_container(&source.id),
            "fixture source identity is not canonical"
        );
        self.source_id = Some(source.id.clone());
        let source_state = self.docker.inspect_container(&source.id, None).await?;
        ensure!(
            source_state.state.and_then(|state| state.running) == Some(false),
            "fixture source was unexpectedly started"
        );
        let (repository, tag) = self
            .image_reference
            .rsplit_once(':')
            .context("fixture image reference lacks its unique tag")?;
        let image = self
            .docker
            .commit_container(
                CommitContainerOptionsBuilder::new()
                    .container(&source.id)
                    .repo(repository)
                    .tag(tag)
                    .pause(false)
                    .changes(&format!("VOLUME [\"{IMAGE_VOLUME}\"]"))
                    .build(),
                ContainerConfig {
                    labels: Some(self.owned_labels()),
                    ..Default::default()
                },
            )
            .await?;
        ensure!(
            canonical_image(&image.id) && image.id != self.base_id,
            "fixture commit did not return a distinct canonical image"
        );
        self.image_id = Some(image.id.clone());
        let inspected = self.docker.inspect_image(&image.id).await?;
        ensure!(
            inspected
                .config
                .and_then(|config| config.volumes)
                .is_some_and(|volumes| volumes.len() == 1 && volumes.iter().any(|path| path == IMAGE_VOLUME)),
            "fixture image does not declare exactly its intended anonymous volume"
        );

        ensure!(
            missing(&self.docker.inspect_volume(&self.runtime.volume_name).await),
            "fixture named runtime volume already exists or cannot be inspected"
        );
        let volume = self
            .docker
            .create_volume(VolumeCreateRequest {
                name: Some(self.runtime.volume_name.clone()),
                driver: Some("local".into()),
                labels: Some(self.owned_labels()),
                ..Default::default()
            })
            .await?;
        ensure!(
            volume.name == self.runtime.volume_name,
            "fixture named volume identity changed"
        );
        self.runtime.validate_volume(&self.docker, &volume.name).await?;
        ensure!(
            missing(&self.docker.inspect_container(&self.runtime.seed_name, None).await),
            "fixture seed writer name already exists or cannot be inspected"
        );
        let writer = self
            .docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::new()
                        .name(&self.runtime.seed_name)
                        .build(),
                ),
                ContainerCreateBody {
                    image: Some(image.id),
                    entrypoint: Some(Vec::new()),
                    cmd: Some(vec!["false".into()]),
                    labels: Some(self.owned_labels()),
                    host_config: Some(HostConfig {
                        mounts: Some(vec![Mount {
                            typ: Some(MountTypeEnum::VOLUME),
                            source: Some(volume.name),
                            target: Some("/.runtime-seed-fixture".into()),
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
            canonical_container(&writer.id),
            "fixture seed writer identity is not canonical"
        );
        self.writer_id = Some(writer.id.clone());
        self.capture_writer(&writer.id).await?;
        ensure!(
            self.anonymous.len() == 1,
            "exact stopped writer did not allocate its image-declared anonymous volume"
        );
        eprintln!(
            "runtime seed fixture ownership: source_container_id={} seed_container_id={} anonymous_volume={} named_runtime_volume={} fixture_image_id={}",
            source.id,
            writer.id,
            self.anonymous[0],
            self.runtime.volume_name,
            self.image_id.as_deref().context("fixture image identity missing")?
        );
        Ok(())
    }

    async fn capture_writer(&mut self, writer: &str) -> anyhow::Result<()> {
        let inspected = self.docker.inspect_container(writer, None).await?;
        ensure!(
            inspected.id.as_deref() == Some(writer)
                && inspected.state.as_ref().and_then(|state| state.running) == Some(false)
                && inspected
                    .config
                    .as_ref()
                    .and_then(|config| config.labels.as_ref())
                    .is_some_and(|actual| self
                        .owned_labels()
                        .iter()
                        .all(|(key, value)| actual.get(key) == Some(value))),
            "exact stopped writer ownership or state changed"
        );
        let mounts = inspected.mounts.context("exact writer lacks native mount evidence")?;
        ensure!(
            mounts.iter().any(|mount| mount.typ == Some(MountPointTypeEnum::VOLUME)
                && mount.name.as_deref() == Some(self.runtime.volume_name.as_str())
                && mount.destination.as_deref() == Some("/.runtime-seed-fixture")
                && mount.rw == Some(true)),
            "exact writer lacks its named runtime volume"
        );
        for mount in mounts {
            if mount.typ != Some(MountPointTypeEnum::VOLUME)
                || mount.name.as_deref() == Some(self.runtime.volume_name.as_str())
            {
                continue;
            }
            let name = mount.name.context("anonymous mount lacks its native volume name")?;
            ensure!(
                canonical_container(&name) && mount.destination.as_deref() == Some(IMAGE_VOLUME),
                "writer anonymous volume evidence is unexpected"
            );
            if !self.anonymous.contains(&name) {
                self.anonymous.push(name);
            }
        }
        Ok(())
    }

    async fn observe_removal(&self) -> anyhow::Result<Observation> {
        let removal_succeeded = self.runtime.remove_seed_writer(&self.docker).await.is_ok();
        let writer = self
            .writer_id
            .as_deref()
            .context("exact seed writer identity missing")?;
        let writer_absent = missing(&self.docker.inspect_container(writer, None).await);
        let mut anonymous_absent = true;
        for name in &self.anonymous {
            anonymous_absent &= missing(&self.docker.inspect_volume(name).await);
        }
        let named_survived = self
            .runtime
            .validate_volume(&self.docker, &self.runtime.volume_name)
            .await
            .is_ok();
        Ok(Observation {
            removal_succeeded,
            writer_absent,
            anonymous_absent,
            named_survived,
        })
    }

    async fn remove_exact_container(&mut self, writer: bool) -> anyhow::Result<()> {
        let retained = if writer {
            self.writer_id.clone()
        } else {
            self.source_id.clone()
        };
        let requested = retained.as_deref().unwrap_or(if writer {
            &self.runtime.seed_name
        } else {
            &self.source_name
        });
        let inspected = match self.docker.inspect_container(requested, None).await {
            Ok(inspected) => inspected,
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let id = inspected
            .id
            .context("fixture cleanup lacks canonical container identity")?;
        ensure!(
            canonical_container(&id)
                && retained.as_ref().is_none_or(|expected| expected == &id)
                && inspected.state.as_ref().and_then(|state| state.running) == Some(false)
                && inspected
                    .config
                    .as_ref()
                    .and_then(|config| config.labels.as_ref())
                    .is_some_and(|actual| self
                        .owned_labels()
                        .iter()
                        .all(|(key, value)| actual.get(key) == Some(value))),
            "fixture cleanup container ownership or state changed"
        );
        if writer {
            self.capture_writer(&id).await?;
        }
        self.docker
            .remove_container(
                &id,
                Some(RemoveContainerOptionsBuilder::new().force(false).v(true).build()),
            )
            .await?;
        ensure!(
            missing(&self.docker.inspect_container(&id, None).await),
            "exact fixture container absence remains unconfirmed"
        );
        Ok(())
    }

    async fn cleanup(&mut self) -> anyhow::Result<()> {
        self.remove_exact_container(true).await?;
        for name in &self.anonymous {
            match self.docker.inspect_volume(name).await {
                Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => continue,
                Err(error) => return Err(error.into()),
                Ok(volume) => ensure!(volume.name == *name, "retained anonymous volume identity changed"),
            }
            self.docker
                .remove_volume(name, Some(RemoveVolumeOptionsBuilder::new().force(false).build()))
                .await?;
            ensure!(
                missing(&self.docker.inspect_volume(name).await),
                "exact anonymous fixture volume absence remains unconfirmed"
            );
        }
        self.remove_exact_container(false).await?;
        match self.docker.inspect_volume(&self.runtime.volume_name).await {
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {}
            Err(error) => return Err(error.into()),
            Ok(_) => {
                self.runtime
                    .validate_volume(&self.docker, &self.runtime.volume_name)
                    .await?;
                self.docker
                    .remove_volume(
                        &self.runtime.volume_name,
                        Some(RemoveVolumeOptionsBuilder::new().force(false).build()),
                    )
                    .await?;
                ensure!(
                    missing(&self.docker.inspect_volume(&self.runtime.volume_name).await),
                    "exact named fixture volume absence remains unconfirmed"
                );
            }
        }
        let inspected = self
            .docker
            .inspect_image(self.image_id.as_deref().unwrap_or(&self.image_reference))
            .await;
        match inspected {
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {}
            Err(error) => return Err(error.into()),
            Ok(image) => {
                let id = image.id.context("fixture image cleanup lacks canonical identity")?;
                ensure!(
                    canonical_image(&id)
                        && id != self.base_id
                        && self.image_id.as_ref().is_none_or(|expected| expected == &id)
                        && image
                            .config
                            .as_ref()
                            .and_then(|config| config.labels.as_ref())
                            .is_some_and(|actual| self
                                .owned_labels()
                                .iter()
                                .all(|(key, value)| actual.get(key) == Some(value))),
                    "fixture image cleanup ownership changed"
                );
                self.docker
                    .remove_image(
                        &id,
                        Some(RemoveImageOptionsBuilder::new().force(false).noprune(true).build()),
                        None,
                    )
                    .await?;
                ensure!(
                    missing(&self.docker.inspect_image(&id).await),
                    "exact local fixture image absence remains unconfirmed"
                );
            }
        }
        ensure!(
            self.docker.inspect_image(&self.base_id).await.is_ok(),
            "cached shared base image was not preserved"
        );
        eprintln!(
            "runtime seed fixture cleanup: source_container_absent=true seed_container_absent=true anonymous_volumes_absent=true named_runtime_volume_absent=true fixture_image_absent=true shared_base_preserved=true"
        );
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker and a cached node:22-bookworm-slim; run with --ignored"]
async fn seed_writer_removal_retires_image_anonymous_volumes_and_preserves_named_runtime() -> anyhow::Result<()> {
    let mut fixture = Fixture::new().await?;
    let observed = fixture.observe_removal().await;
    let cleanup = fixture.cleanup().await;
    cleanup?;
    let observed = observed?;
    eprintln!(
        "runtime seed removal boundary: removal_succeeded={} seed_container_absent={} anonymous_volumes_absent={} named_runtime_volume_survived={}",
        observed.removal_succeeded, observed.writer_absent, observed.anonymous_absent, observed.named_survived
    );
    ensure!(
        observed.removal_succeeded && observed.writer_absent,
        "actual seed removal did not confirm exact container absence"
    );
    ensure!(
        observed.named_survived,
        "seed removal destroyed the named runtime volume"
    );
    ensure!(
        observed.anonymous_absent,
        "seed removal left an exact image-created anonymous volume behind"
    );
    Ok(())
}
