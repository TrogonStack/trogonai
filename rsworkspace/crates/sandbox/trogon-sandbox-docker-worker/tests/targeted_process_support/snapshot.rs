// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, ensure};
use bollard::Docker;
use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, DownloadFromContainerOptionsBuilder, EventsOptionsBuilder,
    RemoveContainerOptionsBuilder, RemoveImageOptionsBuilder,
};
use futures::StreamExt;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{ArtifactType, Digest, OciArtifactRef};
use trogon_sandbox_docker_worker::snapshot::{ROOTFS_LAYER_MEDIA_TYPE, gzip_tar};

use crate::targeted_process_support::{IMAGE, Native, fresh_id};

const ROOTFS_BOUND: usize = 512 * 1024 * 1024;
const COMPRESSED_BOUND: usize = 128 * 1024 * 1024;
const OWNER: &str = "sandbox.trogon.live/owner";
const LEASE: &str = "sandbox.trogon.live/lease-id";
const MARKER: &str = "/tmp/rollback-snapshot-control";

struct Blob {
    body: Vec<u8>,
    digest: String,
    content_type: &'static str,
}

fn digest(bytes: &[u8]) -> String {
    let hex: String = Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

#[derive(Default)]
struct Effects {
    mutations: Vec<(String, String)>,
    barriers: HashMap<String, (String, Option<String>, bool)>,
    failed: bool,
}

pub struct NativeEffects {
    state: Arc<Mutex<Effects>>,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl NativeEffects {
    pub fn start(docker: &Docker, owner: String) -> Self {
        let state = Arc::new(Mutex::new(Effects::default()));
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let (state, cancel) = (state.clone(), cancel.clone());
            let docker = docker.clone().with_timeout(Duration::from_secs(600));
            async move {
                let filters: HashMap<String, Vec<String>> = HashMap::from([
                    ("type".into(), vec!["container".into()]),
                    ("label".into(), vec![format!("{OWNER}={owner}")]),
                ]);
                let since = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    .to_string();
                let mut stream = docker.events(Some(
                    EventsOptionsBuilder::new().since(&since).filters(&filters).build(),
                ));
                loop {
                    let event = tokio::select! {
                        _ = cancel.cancelled() => break,
                        event = stream.next() => event,
                    };
                    let Some(Ok(event)) = event else {
                        state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).failed = true;
                        break;
                    };
                    let (Some(action), Some(actor)) = (event.action, event.actor) else {
                        continue;
                    };
                    let labels = actor.attributes.unwrap_or_default();
                    if labels.get(OWNER) != Some(&owner) {
                        continue;
                    }
                    let Some(id) = actor
                        .id
                        .filter(|id| id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
                    else {
                        state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).failed = true;
                        break;
                    };
                    let mut state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    if state.mutations.len() >= 64 {
                        state.failed = true;
                        break;
                    }
                    match action.as_str() {
                        "create" | "destroy" => state.mutations.push((id, action)),
                        _ => {
                            for (marker, (container, exec, completed)) in &mut state.barriers {
                                if container != &id {
                                    continue;
                                }
                                if action.starts_with("exec_create:") && action.contains(marker.as_str()) {
                                    *exec = labels.get("execID").cloned();
                                } else if action == "exec_die"
                                    && exec.is_some()
                                    && labels.get("execID") == exec.as_ref()
                                {
                                    *completed = true;
                                }
                            }
                        }
                    }
                }
            }
        });
        Self {
            state,
            cancel,
            task: Some(task),
        }
    }

    pub fn checkpoint(&self) -> anyhow::Result<Vec<(String, String)>> {
        let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        ensure!(!state.failed, "owned native effect observer failed");
        Ok(state.mutations.clone())
    }

    pub fn register_barrier(&self, marker: &str, container: &str) -> anyhow::Result<()> {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        ensure!(
            state.barriers.len() < 4 && !state.barriers.contains_key(marker),
            "native effect barrier bound exceeded"
        );
        state.barriers.insert(marker.into(), (container.into(), None, false));
        Ok(())
    }

    pub async fn wait_for_barrier(&self, marker: &str) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                self.checkpoint()?;
                if self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .barriers
                    .get(marker)
                    .is_some_and(|(_, _, completed)| *completed)
                {
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("owned native effect barrier deadline")??;
        Ok(())
    }

    pub async fn stop(&mut self) -> anyhow::Result<()> {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(5), task).await??;
        }
        self.checkpoint()?;
        Ok(())
    }
}

impl Drop for NativeEffects {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub struct SnapshotFixture {
    pub reference: OciArtifactRef,
    marker: String,
    base_id: String,
    image_tag: String,
    imported_id: Option<String>,
    requests: Arc<AtomicUsize>,
    errors: Arc<AtomicUsize>,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl SnapshotFixture {
    pub async fn prepare(docker: &Docker, native: &mut Native, owner: &str) -> anyhow::Result<Self> {
        let base_id = docker
            .inspect_image(IMAGE)
            .await?
            .id
            .context("cached immutable Node image required")?;
        let marker = fresh_id();
        let lease = format!("rollback-snapshot-source-{marker}");
        native.register_lease(&lease);
        let created = docker
            .create_container(
                Some(CreateContainerOptionsBuilder::new().name(&lease).build()),
                ContainerCreateBody {
                    image: Some(base_id.clone()),
                    entrypoint: Some(vec!["node".into()]),
                    cmd: Some(vec![
                        "-e".into(),
                        format!("require('fs').writeFileSync('{MARKER}','{marker}');setInterval(()=>{{}},1000)"),
                    ]),
                    labels: Some(HashMap::from([
                        (OWNER.into(), owner.into()),
                        (LEASE.into(), lease.clone()),
                    ])),
                    host_config: Some(HostConfig {
                        network_mode: Some("none".into()),
                        cap_drop: Some(vec!["ALL".into()]),
                        security_opt: Some(vec!["no-new-privileges:true".into()]),
                        memory: Some(256 * 1024 * 1024),
                        pids_limit: Some(64),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        native.register_id(&created.id);
        docker.start_container(&created.id, None).await?;
        native.running(&created.id, &lease).await?;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match read_marker(docker, &created.id).await {
                    Ok(found) if found == marker => return Ok::<_, anyhow::Error>(()),
                    Ok(_) => anyhow::bail!("owned snapshot source marker changed"),
                    Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
                }
            }
        })
        .await
        .context("snapshot source marker readiness deadline")??;
        let raw = tokio::time::timeout(Duration::from_secs(60), async {
            let mut stream = docker.export_container(&created.id);
            let mut raw = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                ensure!(
                    raw.len().saturating_add(chunk.len()) <= ROOTFS_BOUND,
                    "snapshot rootfs export exceeds fixture bound"
                );
                raw.extend_from_slice(&chunk);
            }
            ensure!(!raw.is_empty(), "snapshot source export is empty");
            Ok::<_, anyhow::Error>(raw)
        })
        .await
        .context("owned snapshot export deadline")??;
        native.running(&created.id, &lease).await?;
        docker
            .remove_container(
                &created.id,
                Some(RemoveContainerOptionsBuilder::new().force(true).v(true).build()),
            )
            .await?;
        ensure!(
            native.absent(&created.id).await?,
            "exact snapshot source native absence unconfirmed"
        );
        let compressed = tokio::task::spawn_blocking(move || gzip_tar(&raw)).await??;
        ensure!(
            compressed.len() <= COMPRESSED_BOUND,
            "snapshot compressed rootfs exceeds fixture bound"
        );
        let layer_digest = digest(&compressed);
        let config = b"{}".to_vec();
        let config_digest = digest(&config);
        let manifest = serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "artifactType": ArtifactType::SandboxRootfsV1.media_type(),
            "config": {"mediaType": "application/vnd.oci.empty.v1+json", "digest": config_digest, "size": config.len()},
            "layers": [{"mediaType": ROOTFS_LAYER_MEDIA_TYPE, "digest": layer_digest, "size": compressed.len()}]
        }))?;
        let manifest_digest = digest(&manifest);
        let image_tag = format!("trogon-sandbox-restore:restored-{}", manifest_digest.replace(':', "-"));
        ensure!(
            missing(&docker.inspect_image(&image_tag).await),
            "unique snapshot import tag already exists"
        );
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let reference = OciArtifactRef {
            registry: listener.local_addr()?.to_string(),
            repository: "snapshots/assigned-rollback".into(),
            digest: Digest::new(&manifest_digest)?,
            artifact_type: ArtifactType::SandboxRootfsV1,
        };
        let prefix = format!("/v2/{}", reference.repository);
        let blobs = Arc::new(HashMap::from([
            (
                "/v2/".into(),
                Blob {
                    body: b"{}".to_vec(),
                    digest: config_digest.clone(),
                    content_type: "application/json",
                },
            ),
            (
                format!("{prefix}/manifests/{manifest_digest}"),
                Blob {
                    body: manifest,
                    digest: manifest_digest,
                    content_type: "application/vnd.oci.image.manifest.v1+json",
                },
            ),
            (
                format!("{prefix}/blobs/{config_digest}"),
                Blob {
                    body: config,
                    digest: config_digest,
                    content_type: "application/vnd.oci.empty.v1+json",
                },
            ),
            (
                format!("{prefix}/blobs/{layer_digest}"),
                Blob {
                    body: compressed,
                    digest: layer_digest,
                    content_type: ROOTFS_LAYER_MEDIA_TYPE,
                },
            ),
        ]));
        let requests = Arc::new(AtomicUsize::new(0));
        let errors = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let (requests, errors, cancel) = (requests.clone(), errors.clone(), cancel.clone());
            async move {
                let mut connections = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        accepted = listener.accept(), if connections.len() < 8 => {
                            let Ok((stream, peer)) = accepted else { errors.fetch_add(1, Ordering::SeqCst); break; };
                            if !peer.ip().is_loopback() { errors.fetch_add(1, Ordering::SeqCst); break; }
                            let (blobs, requests) = (blobs.clone(), requests.clone());
                            connections.spawn(async move {
                                tokio::time::timeout(Duration::from_secs(30), serve(stream, &blobs, &requests)).await?
                            });
                        }
                        completed = connections.join_next(), if !connections.is_empty() => {
                            if !matches!(completed, Some(Ok(Ok(())))) { errors.fetch_add(1, Ordering::SeqCst); }
                        }
                    }
                }
                connections.abort_all();
                while connections.join_next().await.is_some() {}
            }
        });
        Ok(Self {
            reference,
            marker,
            base_id,
            image_tag,
            imported_id: None,
            requests,
            errors,
            cancel,
            task: Some(task),
        })
    }

    pub fn reads(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    pub fn marker(&self) -> &str {
        &self.marker
    }

    pub async fn seal_import(&mut self, docker: &Docker) -> anyhow::Result<String> {
        let image = docker.inspect_image(&self.image_tag).await?;
        let id = image.id.context("snapshot import image identity absent")?;
        ensure!(id != self.base_id, "snapshot control did not import its unique rootfs");
        self.imported_id = Some(id.clone());
        Ok(id)
    }

    pub async fn finish(&mut self, docker: &Docker) -> anyhow::Result<()> {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(5), task).await??;
        }
        match docker.inspect_image(&self.image_tag).await {
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {}
            Err(error) => return Err(error.into()),
            Ok(image) => {
                let id = image.id.context("owned imported image has no canonical ID")?;
                ensure!(
                    self.imported_id.as_ref().is_none_or(|expected| expected == &id)
                        && image
                            .repo_tags
                            .as_ref()
                            .is_some_and(|tags| tags == &vec![self.image_tag.clone()]),
                    "snapshot fixture refuses changed imported-image ownership"
                );
                docker
                    .remove_image(
                        &id,
                        Some(RemoveImageOptionsBuilder::new().force(false).noprune(true).build()),
                        None,
                    )
                    .await?;
                ensure!(
                    missing(&docker.inspect_image(&id).await) && missing(&docker.inspect_image(&self.image_tag).await),
                    "exact owned snapshot import image absence unconfirmed"
                );
            }
        }
        ensure!(
            docker.inspect_image(&self.base_id).await?.id.as_ref() == Some(&self.base_id),
            "snapshot fixture changed cached base-image custody"
        );
        ensure!(
            self.errors.load(Ordering::SeqCst) == 0,
            "owned OCI fixture transport failed"
        );
        eprintln!(
            "assigned rollback snapshot fixture cleanup: server_joined=true blobs_released=true exact_imported_image_absent=true cached_base_preserved=true"
        );
        Ok(())
    }
}

impl Drop for SnapshotFixture {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub fn missing<T>(result: &Result<T, bollard::errors::Error>) -> bool {
    matches!(
        result,
        Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
    )
}

async fn read_marker(docker: &Docker, container: &str) -> anyhow::Result<String> {
    let mut stream = docker.download_from_container(
        container,
        Some(DownloadFromContainerOptionsBuilder::new().path(MARKER).build()),
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= 16 * 1024,
            "snapshot source marker archive exceeds bound"
        );
        bytes.extend_from_slice(&chunk);
    }
    let mut archive = tar::Archive::new(bytes.as_slice());
    let mut entries = archive.entries()?;
    let mut entry = entries.next().context("snapshot marker absent")??;
    ensure!(
        entry.header().entry_type().is_file() && entry.size() <= 64,
        "snapshot marker metadata changed"
    );
    let mut marker = String::new();
    entry.read_to_string(&mut marker)?;
    drop(entry);
    ensure!(
        entries.next().is_none(),
        "snapshot marker archive carries extra entries"
    );
    Ok(marker)
}

async fn serve(mut stream: TcpStream, blobs: &HashMap<String, Blob>, requests: &AtomicUsize) -> anyhow::Result<()> {
    ensure!(
        requests.fetch_add(1, Ordering::SeqCst) < 128,
        "OCI fixture request-count bound exceeded"
    );
    let mut header = Vec::new();
    loop {
        let mut chunk = [0; 1024];
        let count = stream.read(&mut chunk).await?;
        ensure!(
            count > 0 && header.len().saturating_add(count) <= 16 * 1024,
            "OCI fixture request-header bound exceeded"
        );
        header.extend_from_slice(&chunk[..count]);
        if let Some(end) = header.windows(4).position(|window| window == b"\r\n\r\n") {
            ensure!(end + 4 == header.len(), "OCI fixture request body is forbidden");
            break;
        }
    }
    let header = std::str::from_utf8(&header)?;
    for line in header.lines().skip(1).filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').context("OCI fixture malformed request header")?;
        ensure!(
            !name.eq_ignore_ascii_case("transfer-encoding")
                && (!name.eq_ignore_ascii_case("content-length") || value.trim() == "0"),
            "OCI fixture request body is forbidden"
        );
    }
    let mut line = header
        .lines()
        .next()
        .context("OCI request line absent")?
        .split_whitespace();
    let method = line.next().context("OCI method absent")?;
    let path = line.next().context("OCI path absent")?;
    ensure!(
        matches!(method, "GET" | "HEAD") && line.next() == Some("HTTP/1.1") && line.next().is_none(),
        "OCI fixture accepts bounded read-only HTTP/1.1 only"
    );
    let blob = blobs
        .get(path)
        .context("OCI request escaped immutable fixture namespace")?;
    stream.write_all(format!("HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: {}\r\nContent-Length: {}\r\nDocker-Distribution-Api-Version: registry/2.0\r\nDocker-Content-Digest: {}\r\n\r\n", blob.content_type, blob.body.len(), blob.digest).as_bytes()).await?;
    if method == "GET" {
        stream.write_all(&blob.body).await?;
    }
    stream.shutdown().await?;
    Ok(())
}
