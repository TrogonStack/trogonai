//! Docker Engine API access via `bollard`.

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use bollard::container::{CreateCheckpointOptions, ListCheckpointsOptions};
use bollard::models::{ContainerConfig, ContainerCreateBody, ContainerStateStatusEnum, HostConfig, PortBinding};
use bollard::query_parameters::{
    CommitContainerOptionsBuilder, CreateContainerOptionsBuilder, CreateImageOptionsBuilder, InspectContainerOptions,
    KillContainerOptions, ListContainersOptionsBuilder, RemoveContainerOptionsBuilder, StartContainerOptions,
    WaitContainerOptions,
};
use bollard::{API_DEFAULT_VERSION, Docker, body_full};
use bytes::Bytes;
use futures::StreamExt;
use http_body_util::{BodyExt, Full};
use hyper::{Method, Request};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use hyperlocal::UnixConnector;

use crate::docker::constants::{DEFAULT_DOCKER_SOCKET, ENGINE_API_VERSION, REQUEST_TIMEOUT_SECS};
use crate::{
    CheckpointCreateSpec, CheckpointRestoreSpec, ContainerRunSpec, ContainerStartResult, DockerEngine, DockerError,
    InspectStatus,
};

/// Docker refuses a container that asks for more cores than the host has, and
/// callers derive `cpu` from a budget rather than a core count.
fn container_cpus(requested: f32, host_cpus: f32) -> f32 {
    requested.min(host_cpus).max(0.1)
}

fn host_cpus() -> f32 {
    std::thread::available_parallelism().map_or(1.0, |n| n.get() as f32)
}

fn non_empty<T>(items: &[T]) -> Option<Vec<T>>
where
    T: Clone,
{
    if items.is_empty() { None } else { Some(items.to_vec()) }
}

pub struct BollardEngine {
    client: Docker,
    socket_path: PathBuf,
}

impl BollardEngine {
    /// Connects the way the Docker CLI does (`DOCKER_HOST`, then the default socket).
    /// Checkpoint restores always go to the default socket.
    pub fn connect() -> Result<Self, DockerError> {
        let client = Docker::connect_with_defaults().map_err(DockerError::Connect)?;
        Ok(Self {
            client,
            socket_path: PathBuf::from(DEFAULT_DOCKER_SOCKET),
        })
    }

    /// Connects to the daemon listening on the unix socket at `socket_path`.
    pub fn connect_unix(socket_path: impl Into<PathBuf>) -> Result<Self, DockerError> {
        let socket_path = socket_path.into();
        let path = socket_path.to_str().ok_or_else(|| DockerError::NonUtf8Path {
            what: "docker socket",
            path: socket_path.clone(),
        })?;
        let client =
            Docker::connect_with_unix(path, REQUEST_TIMEOUT_SECS, API_DEFAULT_VERSION).map_err(DockerError::Connect)?;
        Ok(Self { client, socket_path })
    }

    pub fn client(&self) -> &Docker {
        &self.client
    }

    fn container_config(spec: &ContainerRunSpec) -> ContainerCreateBody {
        let memory_bytes = (spec.memory_mib as i64).saturating_mul(1024 * 1024);
        let nano_cpus = (container_cpus(spec.cpu, host_cpus()) * 1_000_000_000.0) as i64;

        let host_config = HostConfig {
            memory: Some(memory_bytes),
            memory_swap: spec.hardening.disable_swap.then_some(memory_bytes),
            pids_limit: spec.hardening.pids_limit,
            readonly_rootfs: spec.hardening.readonly_rootfs.then_some(true),
            security_opt: spec
                .hardening
                .no_new_privileges
                .then(|| vec!["no-new-privileges".to_string()]),
            runtime: spec.hardening.runtime.clone(),
            shm_size: spec
                .hardening
                .shm_size_mib
                .map(|mib| (mib as i64).saturating_mul(1024 * 1024)),
            tmpfs: non_empty(&spec.hardening.tmpfs).map(|mounts| mounts.into_iter().collect()),
            nano_cpus: Some(nano_cpus),
            storage_opt: spec
                .disk_mib
                .map(|mib| HashMap::from([("size".to_string(), format!("{mib}m"))])),
            network_mode: Some(spec.network_mode.clone()),
            binds: non_empty(&spec.binds),
            cap_add: non_empty(&spec.cap_add),
            cap_drop: non_empty(&spec.cap_drop),
            port_bindings: non_empty(&spec.exposed_ports).map(|ports| {
                ports
                    .iter()
                    .map(|port| {
                        (
                            format!("{port}/tcp"),
                            Some(vec![PortBinding {
                                host_ip: Some("127.0.0.1".to_string()),
                                host_port: Some("0".to_string()),
                            }]),
                        )
                    })
                    .collect()
            }),
            auto_remove: Some(true),
            ..Default::default()
        };

        ContainerCreateBody {
            image: Some(spec.image.clone()),
            env: non_empty(&spec.env),
            entrypoint: spec.entrypoint_override.clone(),
            cmd: spec.cmd_override.clone(),
            working_dir: spec.working_dir.clone(),
            user: spec.user.clone(),
            exposed_ports: non_empty(&spec.exposed_ports)
                .map(|ports| ports.iter().map(|port| format!("{port}/tcp")).collect()),
            labels: if spec.labels.is_empty() {
                None
            } else {
                Some(spec.labels.clone().into_iter().collect())
            },
            host_config: Some(host_config),
            ..Default::default()
        }
    }

    /// Resolves each of `requested_ports` to its bound `127.0.0.1:{host_port}`
    /// endpoint from `NetworkSettings.ports`, skipping any port Docker did not
    /// bind rather than erroring the whole launch.
    fn resolve_port_endpoints(requested_ports: &[u16], ports: &bollard::models::PortMap) -> HashMap<u16, String> {
        let mut endpoints = HashMap::new();
        for port in requested_ports {
            let key = format!("{port}/tcp");
            let Some(Some(bindings)) = ports.get(&key) else {
                continue;
            };
            let Some(host_port) = bindings.first().and_then(|b| b.host_port.as_ref()) else {
                continue;
            };
            endpoints.insert(*port, format!("127.0.0.1:{host_port}"));
        }
        endpoints
    }

    /// Blocks until the container exits and returns its exit status code.
    pub async fn wait_container_exit(&self, container_id: &str) -> Result<i64, DockerError> {
        let mut wait = self.client.wait_container(container_id, None::<WaitContainerOptions>);
        let result = wait
            .next()
            .await
            .ok_or(DockerError::WaitReturnedNothing)?
            .map_err(DockerError::api("wait_container"))?;
        Ok(result.status_code)
    }

    /// Whether the daemon reports experimental mode enabled (needed for checkpoints).
    pub async fn experimental_enabled(&self) -> Result<bool, DockerError> {
        let info = self.client.info().await.map_err(DockerError::api("info"))?;
        Ok(info.experimental_build.unwrap_or(false))
    }

    /// POST `/containers/{id}/start?checkpoint=...&checkpoint-dir=...`
    async fn start_container_with_checkpoint(
        &self,
        container_id: &str,
        spec: &CheckpointRestoreSpec,
    ) -> Result<(), DockerError> {
        let query = serde_urlencoded::to_string([
            ("checkpoint", spec.checkpoint_id.as_str()),
            ("checkpoint-dir", spec.checkpoint_dir.as_str()),
        ])
        .map_err(DockerError::CheckpointQuery)?;
        let path = format!("/{ENGINE_API_VERSION}/containers/{container_id}/start?{query}");

        let client: Client<UnixConnector, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(UnixConnector);
        let request = Request::builder()
            .method(Method::POST)
            .uri(hyperlocal::Uri::new(&self.socket_path, &path))
            .body(Full::new(Bytes::new()))
            .map_err(DockerError::CheckpointRequestBuild)?;

        let response = client.request(request).await.map_err(DockerError::CheckpointRequest)?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .into_body()
                .collect()
                .await
                .map(|b| String::from_utf8_lossy(&b.to_bytes()).into_owned())
                .unwrap_or_default();
            return Err(DockerError::CheckpointRejected { status, body });
        }
        Ok(())
    }
}

#[async_trait]
impl DockerEngine for BollardEngine {
    async fn create_and_start(&self, spec: ContainerRunSpec) -> Result<ContainerStartResult, DockerError> {
        let exposed_ports = spec.exposed_ports.clone();
        let container_id = self.create_container_stopped(spec).await?;
        self.client
            .start_container(&container_id, None::<StartContainerOptions>)
            .await
            .map_err(DockerError::api("start_container"))?;

        let port_endpoints = if exposed_ports.is_empty() {
            HashMap::new()
        } else {
            let detail = self
                .client
                .inspect_container(&container_id, None::<InspectContainerOptions>)
                .await
                .map_err(DockerError::api("inspect_container"))?;
            detail
                .network_settings
                .and_then(|settings| settings.ports)
                .map(|ports| Self::resolve_port_endpoints(&exposed_ports, &ports))
                .unwrap_or_default()
        };

        Ok(ContainerStartResult {
            container_id,
            port_endpoints,
        })
    }

    async fn create_container_stopped(&self, spec: ContainerRunSpec) -> Result<String, DockerError> {
        let options = CreateContainerOptionsBuilder::default().name(&spec.name).build();

        let create = self
            .client
            .create_container(Some(options), Self::container_config(&spec))
            .await
            .map_err(DockerError::api("create_container"))?;

        Ok(create.id)
    }

    async fn inspect(&self, container_id: &str) -> Result<InspectStatus, DockerError> {
        let detail = self
            .client
            .inspect_container(container_id, None::<InspectContainerOptions>)
            .await
            .map_err(DockerError::api("inspect_container"))?;

        let state = detail.state.ok_or(DockerError::ContainerStateMissing)?;

        Ok(match state.status {
            Some(ContainerStateStatusEnum::RUNNING) => InspectStatus::Running,
            Some(ContainerStateStatusEnum::CREATED)
            | Some(ContainerStateStatusEnum::RESTARTING)
            | Some(ContainerStateStatusEnum::PAUSED) => InspectStatus::Pending,
            Some(ContainerStateStatusEnum::EXITED) => InspectStatus::Exited(state.exit_code.unwrap_or(-1) as i32),
            Some(ContainerStateStatusEnum::REMOVING) | Some(ContainerStateStatusEnum::DEAD) => {
                InspectStatus::Failed("container is dead".into())
            }
            other => InspectStatus::Failed(format!("unknown docker status: {other:?}")),
        })
    }

    async fn kill(&self, container_id: &str) -> Result<(), DockerError> {
        match self
            .client
            .kill_container(container_id, None::<KillContainerOptions>)
            .await
        {
            Ok(()) => Ok(()),
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => Ok(()),
            Err(error) => Err(DockerError::api("kill_container")(error)),
        }
    }

    async fn list_by_label(&self, key: &str, value: &str) -> Result<Vec<String>, DockerError> {
        let filters = HashMap::from([("label".to_string(), vec![format!("{key}={value}")])]);
        let options = ListContainersOptionsBuilder::default()
            .all(true)
            .filters(&filters)
            .build();
        let listed = self
            .client
            .list_containers(Some(options))
            .await
            .map_err(DockerError::api("list_containers"))?;
        Ok(listed
            .into_iter()
            .filter(|c| {
                c.labels
                    .as_ref()
                    .and_then(|labels| labels.get(key))
                    .is_some_and(|v| v == value)
            })
            .filter_map(|c| c.id)
            .collect())
    }

    async fn remove(&self, container_id: &str) -> Result<(), DockerError> {
        let options = RemoveContainerOptionsBuilder::default().force(true).build();
        match self.client.remove_container(container_id, Some(options)).await {
            Ok(()) => Ok(()),
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => Ok(()),
            Err(error) => Err(DockerError::api("remove_container")(error)),
        }
    }

    async fn commit(&self, container_id: &str, repo: &str, tag: &str) -> Result<String, DockerError> {
        let options = CommitContainerOptionsBuilder::default()
            .container(container_id)
            .repo(repo)
            .tag(tag)
            .build();
        let response = self
            .client
            .commit_container(options, ContainerConfig::default())
            .await
            .map_err(DockerError::api("commit_container"))?;
        Ok(response.id)
    }

    async fn export_rootfs(&self, container_id: &str) -> Result<Vec<u8>, DockerError> {
        let mut stream = self.client.export_container(container_id);
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(DockerError::api("export_container stream"))?;
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }

    async fn import_rootfs(&self, tar: &[u8], repo: &str, tag: &str) -> Result<String, DockerError> {
        let options = CreateImageOptionsBuilder::default()
            .from_src("-")
            .repo(repo)
            .tag(tag)
            .build();
        let mut import_stream =
            self.client
                .create_image(Some(options), Some(body_full(Bytes::copy_from_slice(tar))), None);

        let mut image_id = format!("{repo}:{tag}");
        while let Some(info) = import_stream.next().await {
            let info = info.map_err(DockerError::api("create_image import stream"))?;
            if let Some(id) = info.id {
                image_id = id;
            }
        }
        Ok(image_id)
    }

    async fn create_checkpoint_leave_running(
        &self,
        container_id: &str,
        spec: &CheckpointCreateSpec,
    ) -> Result<(), DockerError> {
        std::fs::create_dir_all(&spec.checkpoint_dir).map_err(|source| DockerError::CheckpointDir {
            dir: spec.checkpoint_dir.clone(),
            source,
        })?;

        let options = CreateCheckpointOptions {
            checkpoint_id: spec.checkpoint_id.clone(),
            checkpoint_dir: Some(spec.checkpoint_dir.clone()),
            exit: false,
        };

        self.client
            .create_checkpoint(container_id, options)
            .await
            .map_err(DockerError::api(
                "create_checkpoint (requires experimental daemon + CRIU)",
            ))?;

        let listed = self
            .client
            .list_checkpoints(
                container_id,
                Some(ListCheckpointsOptions {
                    checkpoint_dir: Some(spec.checkpoint_dir.clone()),
                }),
            )
            .await
            .map_err(DockerError::api("list_checkpoints after create"))?;

        if !listed.iter().any(|c| c.name == spec.checkpoint_id) {
            return Err(DockerError::CheckpointNotVisible {
                checkpoint_id: spec.checkpoint_id.clone(),
            });
        }

        Ok(())
    }

    async fn start_from_checkpoint(&self, container_id: &str, spec: &CheckpointRestoreSpec) -> Result<(), DockerError> {
        // bollard's StartContainerOptions lacks the checkpoint query parameters, so the
        // Engine API call goes out directly on the daemon's unix socket.
        self.start_container_with_checkpoint(container_id, spec).await
    }
}

#[cfg(test)]
mod tests;
