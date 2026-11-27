use super::*;

use trusted_observer_support::{Binding, Observer, Refusal, Response};

/// The observer bound to the first workload container the lane observes.
pub(super) struct BoundSdkObserver {
    pub(super) container: String,
    pub(super) lease: String,
    pub(super) observer: Arc<Observer>,
}

/// A readonly native observer that only serves the worker-created workload it independently witnessed.
pub(super) struct SdkNativeObserver {
    pub(super) docker: Docker,
    pub(super) trace: Arc<Mutex<NativeTrace>>,
    pub(super) owner: String,
    pub(super) image: String,
    pub(super) digest: String,
    pub(super) bound: tokio::sync::Mutex<Option<BoundSdkObserver>>,
}

#[async_trait::async_trait]
impl trusted_observer_support::Host for SdkNativeObserver {
    async fn observe(&self, request: trusted_observer_support::Request) -> Response {
        let id = request.request_id.clone();
        if let Err(code) = request.validate_input() {
            return Response::Refused {
                request_id: String::new(),
                code,
            };
        }
        let mut bound = self.bound.lock().await;
        if bound
            .as_ref()
            .is_some_and(|held| held.container != request.container_id || held.lease != request.lease_id)
        {
            return Response::Refused {
                request_id: id,
                code: Refusal::RequestScope,
            };
        }
        loop {
            let witnessed = {
                let trace = locked(&self.trace);
                if !trace.errors.is_empty() {
                    return Response::Refused {
                        request_id: id,
                        code: Refusal::NativeBindingChanged,
                    };
                }
                trace.containers.get(&request.container_id).is_some_and(|record| {
                    record.lease == request.lease_id
                        && record.role == WORKLOAD_ROLE
                        && record.image_verified
                        && record.isolation_verified
                        && record.artifact_verified
                })
            };
            if witnessed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if self
            .verify_native(&request.container_id, &request.lease_id)
            .await
            .is_err()
        {
            return Response::Refused {
                request_id: id,
                code: Refusal::NativeBindingChanged,
            };
        }
        if bound.is_none() {
            let (retained_capabilities, _) = ContainerHardening::secure().capability_sets(&[]);
            let binding = match Binding::seal_with_retained_capabilities(
                &self.docker,
                request.container_id.clone(),
                request.lease_id.clone(),
                self.owner.clone(),
                self.image.clone(),
                retained_capabilities,
            )
            .await
            {
                Ok(binding) => binding,
                Err(_) => {
                    return Response::Refused {
                        request_id: id,
                        code: Refusal::NativeBindingChanged,
                    };
                }
            };
            let observer = match Observer::create(
                self.docker.clone(),
                binding,
                self.image.clone(),
                format!("{}-sdk-observer", self.owner),
            )
            .await
            {
                Ok(observer) => observer,
                Err(_) => {
                    return Response::Refused {
                        request_id: id,
                        code: Refusal::ObserverChanged,
                    };
                }
            };
            *bound = Some(BoundSdkObserver {
                container: request.container_id.clone(),
                lease: request.lease_id.clone(),
                observer,
            });
        }
        let Some(held) = bound.as_ref() else {
            return Response::Refused {
                request_id: id,
                code: Refusal::ObserverChanged,
            };
        };
        let response = held.observer.observe(request).await;
        if self.verify_native(&held.container, &held.lease).await.is_err() {
            return Response::Refused {
                request_id: id,
                code: Refusal::NativeBindingChanged,
            };
        }
        response
    }

    async fn cleanup(&self) -> anyhow::Result<()> {
        if let Some(held) = self.bound.lock().await.as_ref() {
            held.observer.cleanup().await?;
        }
        Ok(())
    }
}

impl SdkNativeObserver {
    pub(super) async fn verify_native(&self, container: &str, lease: &str) -> anyhow::Result<()> {
        let record = inspect_native(
            &self.docker,
            container,
            &self.owner,
            lease,
            &self.image,
            &self.digest,
            &self.trace,
        )
        .await?;
        ensure!(
            record.role == WORKLOAD_ROLE
                && record.image_verified
                && record.isolation_verified
                && record.artifact_verified,
            "independent observer native provenance remains unconfirmed"
        );
        locked(&self.trace).containers.insert(container.to_owned(), record);
        Ok(())
    }

    pub(super) async fn containers(&self) -> Vec<String> {
        match self.bound.lock().await.as_ref() {
            Some(held) => held.observer.containers().await,
            None => Vec::new(),
        }
    }
}

pub(super) async fn container_absent(docker: &Docker, id: &str) -> anyhow::Result<bool> {
    ensure!(
        canonical(id),
        "exact container absence requires a complete canonical ID"
    );
    match docker.inspect_container(id, None).await {
        Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => Ok(true),
        Ok(found) if found.id.as_deref() == Some(id) => Ok(false),
        _ => anyhow::bail!("exact native container absence is unknown"),
    }
}

pub(super) async fn volume_absent(docker: &Docker, name: &str) -> anyhow::Result<bool> {
    match docker.inspect_volume(name).await {
        Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => Ok(true),
        Ok(found) if found.name == name => Ok(false),
        _ => anyhow::bail!("exact runtime volume absence is unknown"),
    }
}

pub(super) async fn artifact_verified(
    docker: &Docker,
    container: &str,
    path: &str,
    digest: &str,
) -> anyhow::Result<()> {
    let mut stream = docker.download_from_container(
        container,
        Some(DownloadFromContainerOptionsBuilder::new().path(path).build()),
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= 64 * 1024 * 1024,
            "native artifact archive exceeds its bound"
        );
        bytes.extend_from_slice(&chunk);
    }
    let mut archive = tar::Archive::new(Cursor::new(bytes));
    let mut files = 0;
    for entry in archive.entries()? {
        let mut entry = entry?;
        ensure!(
            entry.header().entry_type().is_file()
                && entry.path()?.as_ref() == Path::new("supervisor")
                && entry.header().mode()? == 0o555
                && entry.header().uid()? == 0
                && entry.header().gid()? == 0,
            "native immutable supervisor metadata changed"
        );
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        ensure!(
            hex_sha256(&content) == digest,
            "native supervisor differs from the exact trusted artifact"
        );
        files += 1;
    }
    ensure!(
        files == 1,
        "native supervisor archive must contain exactly its executable"
    );
    Ok(())
}

/// Whether `value` is a sandbox entity ID, the identifier shape the worker uses for runtime registrations.
fn registration_nonce(value: &str) -> bool {
    trogon_sandbox_contract::SandboxEntityId::new(value).is_ok()
}

fn normalized_capability(cap: &str) -> String {
    cap.trim().to_ascii_uppercase().trim_start_matches("CAP_").to_owned()
}

pub(super) async fn inspect_native(
    docker: &Docker,
    id: &str,
    owner: &str,
    lease: &str,
    image: &str,
    digest: &str,
    trace: &Arc<Mutex<NativeTrace>>,
) -> anyhow::Result<NativeRecord> {
    let found = docker.inspect_container(id, None).await?;
    let labels = found
        .config
        .as_ref()
        .and_then(|config| config.labels.as_ref())
        .context("native workload labels are missing")?;
    ensure!(
        found.id.as_deref() == Some(id)
            && labels.get(DOCKER_LABEL_OWNER).map(String::as_str) == Some(owner)
            && labels.get(DOCKER_LABEL_LEASE_ID).map(String::as_str) == Some(lease),
        "native canonical owner and lease controls changed"
    );
    let host = found
        .host_config
        .as_ref()
        .context("native workload host configuration is missing")?;
    let isolation = host.privileged != Some(true)
        && host
            .cap_drop
            .as_ref()
            .is_some_and(|caps| caps.iter().any(|cap| normalized_capability(cap) == "ALL"))
        && host.cap_add.as_ref().is_none_or(|caps| {
            caps.iter()
                .all(|cap| !matches!(normalized_capability(cap).as_str(), "ALL" | "SYS_PTRACE" | "SYS_ADMIN"))
        });
    let volume_prefix = format!("sandbox-command-{digest}-");
    let mount = found
        .mounts
        .as_ref()
        .and_then(|mounts| {
            mounts.iter().find(|mount| {
                mount.typ == Some(bollard::config::MountPointTypeEnum::VOLUME)
                    && mount.rw == Some(false)
                    && mount.name.as_ref().is_some_and(|name| name.starts_with(&volume_prefix))
            })
        })
        .context("native workload lacks its immutable supervisor volume")?;
    let volume = mount
        .name
        .as_ref()
        .context("runtime mount volume identity is missing")?;
    let destination = mount
        .destination
        .as_ref()
        .context("runtime mount destination is missing")?;
    ensure!(
        destination
            .strip_prefix(RUNTIME_MOUNT_PREFIX)
            .is_some_and(registration_nonce),
        "runtime mount is not an unpredictable direct root mount"
    );
    let native_volume = docker.inspect_volume(volume).await?;
    let nonce = native_volume
        .labels
        .get(RUNTIME_NONCE)
        .context("runtime volume registration is missing")?;
    ensure!(
        native_volume.name == *volume
            && native_volume.driver == "local"
            && native_volume.options.is_empty()
            && native_volume.labels.get(DOCKER_LABEL_OWNER).map(String::as_str) == Some(owner)
            && native_volume.labels.get(RUNTIME_DIGEST).map(String::as_str) == Some(digest)
            && registration_nonce(nonce)
            && volume == &format!("sandbox-command-{digest}-{nonce}"),
        "runtime volume ownership or artifact identity changed"
    );
    let mut record = NativeRecord {
        lease: lease.into(),
        role: labels.get(DOCKER_LABEL_ROLE).cloned().unwrap_or_default(),
        image_verified: found.image.as_deref() == Some(image),
        isolation_verified: isolation,
        artifact_verified: false,
        runtime_volume: Some(volume.clone()),
    };
    {
        let mut trace = locked(trace);
        trace.runtime_volumes.insert(volume.clone());
        trace.containers.insert(id.into(), record.clone());
    }
    artifact_verified(docker, id, &format!("{destination}/supervisor"), digest).await?;
    record.artifact_verified = true;
    Ok(record)
}

/// Records every container the worker creates for `owner` and independently verifies its native provenance.
pub(super) fn native_trace(
    docker: &Docker,
    owner: String,
    image: String,
    digest: String,
) -> (Arc<Mutex<NativeTrace>>, CancellationToken, tokio::task::JoinHandle<()>) {
    let trace = Arc::new(Mutex::new(NativeTrace::default()));
    let cancel = CancellationToken::new();
    let state = trace.clone();
    let stop = cancel.clone();
    let docker = docker.clone().with_timeout(Duration::from_secs(600));
    let task = tokio::spawn(async move {
        let filters = HashMap::from([
            ("type".to_owned(), vec!["container".to_owned()]),
            ("event".to_owned(), vec!["create".to_owned()]),
            ("label".to_owned(), vec![format!("{DOCKER_LABEL_OWNER}={owner}")]),
        ]);
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();
        let mut events = docker.events(Some(
            EventsOptionsBuilder::new().since(&since).filters(&filters).build(),
        ));
        loop {
            let event = tokio::select! {
                _ = stop.cancelled() => break,
                event = events.next() => event,
            };
            let Some(Ok(event)) = event else {
                locked(&state).errors.push("native_event_stream_ended");
                break;
            };
            let Some(actor) = event.actor else {
                continue;
            };
            let Some(id) = actor.id.filter(|id| canonical(id)) else {
                continue;
            };
            let labels = actor.attributes.unwrap_or_default();
            if labels.get(DOCKER_LABEL_OWNER) != Some(&owner) {
                continue;
            }
            let Some(lease) = labels.get(DOCKER_LABEL_LEASE_ID).cloned() else {
                if labels.get(RUNTIME_DIGEST) == Some(&digest) {
                    locked(&state).runtime_writers.insert(id);
                }
                continue;
            };
            {
                let mut state = locked(&state);
                if state.containers.len() >= 32 {
                    state.errors.push("native_allocation_bound_exceeded");
                    break;
                }
                state.containers.insert(
                    id.clone(),
                    NativeRecord {
                        lease: lease.clone(),
                        role: labels.get(DOCKER_LABEL_ROLE).cloned().unwrap_or_default(),
                        ..Default::default()
                    },
                );
            }
            match inspect_native(&docker, &id, &owner, &lease, &image, &digest, &state).await {
                Ok(record) => {
                    let mut state = locked(&state);
                    if let Some(volume) = &record.runtime_volume {
                        state.runtime_volumes.insert(volume.clone());
                    }
                    state.containers.insert(id, record);
                }
                Err(_) => {
                    locked(&state).errors.push("native_live_provenance_unconfirmed");
                }
            }
        }
    });
    (trace, cancel, task)
}
