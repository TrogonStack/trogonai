//! A signed worker over one supervised node lease, with probes that census the native process family of a marker.

use super::*;

impl Fixture {
    pub(super) async fn new(bound: bool) -> anyhow::Result<Self> {
        Self::with_capabilities(bound, Vec::new()).await
    }

    pub(super) async fn with_capabilities(bound: bool, capabilities: Vec<ExtraCapability>) -> anyhow::Result<Self> {
        let artifact = ReadEnv::var_os(&SystemEnv, "SANDBOX_DOCKER_COMMAND_SUPERVISOR")
            .context("native cancellation fixture requires SANDBOX_DOCKER_COMMAND_SUPERVISOR")?;
        let docker = bollard::Docker::connect_with_defaults()?;
        docker.info().await?;
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let js = jetstream::new(client.clone());
        let worker = format!("named-control-{}", fresh_id());
        let inner = LocalDockerProvider::with_default_engine(NODE_IMAGE.into())?
            .with_owner(worker.clone())
            .with_hardening(ContainerHardening::secure());
        let runtime = CommandRuntime::load(Path::new(&artifact), worker.clone())?;
        let runtime_digest = runtime.digest().to_owned();
        let runtime_volume = runtime.volume_name().to_owned();
        let runtime_bytes = std::fs::read(&artifact)?;
        let observed_digest: String = sha2::Sha256::digest(&runtime_bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        ensure!(
            observed_digest == runtime_digest,
            "observer artifact changed before native allocation"
        );
        js.create_key_value(jetstream::kv::Config {
            bucket: CUSTODY_BUCKET.into(),
            history: 1,
            max_value_size: 8192,
            ..Default::default()
        })
        .await?;
        let provider = Arc::new(
            docker_provider_over(inner, &client, format!("nats://{}", server.address()), &worker)?
                .with_command_runtime(runtime),
        );
        let mut seed = [0; 32];
        seed[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        seed[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        let signer = DispatchSigner::new("named-control-fixture", seed);
        let verifier: &'static DispatchVerifier = Box::leak(Box::new(
            DispatchVerifier::new(EnforcementMode::Require).with_key(signer.key_id(), &signer.public_key_b64())?,
        ));
        let active: ActiveLeases = Arc::new(Mutex::new(HashMap::new()));
        let mut run = subscribe_with_context(&client, RUN_COMMAND_SUBJECT, verifier).await?;
        let mut cancel = subscribe_with_context(&client, CANCEL_COMMAND_LAUNCH_SUBJECT, verifier).await?;
        let run_client = client.clone();
        let run_provider = provider.clone();
        let run_active = active.clone();
        let run_task = tokio::spawn(async move {
            while let Some(message) = run.next().await {
                let client = run_client.clone();
                let provider = run_provider.clone();
                let active = run_active.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_run_command_verified(message, client, provider, active).await {
                        eprintln!("native run handler failed: {error:#}");
                    }
                });
            }
        });
        let cancel_client = client.clone();
        let cancel_provider = provider.clone();
        let cancel_active = active.clone();
        let cancel_task = tokio::spawn(async move {
            while let Some(message) = cancel.next().await {
                let client = cancel_client.clone();
                let provider = cancel_provider.clone();
                let active = cancel_active.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_cancel_command_launch(message, client, provider, active).await {
                        eprintln!("native cancel handler failed: {error:#}");
                    }
                });
            }
        });
        client.flush().await?;
        let lease = LeaseId::new(fresh_id());
        let mut parts = node_spec_parts()?;
        if !capabilities.is_empty() {
            parts.user = Some("root".into());
            parts.platform.container.extra_capabilities = capabilities;
        }
        let request = node_lease_request_from(lease.as_str(), parts)?;
        let grant = match provider.lease(&request, &lease).await {
            Ok(grant) => grant,
            Err(error) => {
                provider.inner.sweep_owned().await?;
                provider.shutdown_command_runtime().await?;
                return Err(error.into());
            }
        };
        log_owned_container(&grant);
        let payload = ProviderDispatchPayload {
            lease_id: lease.clone(),
            request: request.clone(),
            correlation_id: None,
            resolved_drives: vec![],
            target: None,
        };
        if bound && let Err(error) = provider.bind_command_assignment(&payload).await {
            let cleanup = cleanup_owned_container(&provider, &lease, &grant).await;
            let runtime_cleanup = provider.shutdown_command_runtime().await;
            run_task.abort();
            cancel_task.abort();
            cleanup?;
            runtime_cleanup?;
            return Err(error);
        }
        active.lock().await.insert(lease.clone(), CancellationToken::new());
        let scope = VerbScope {
            tenant: request.tenant.clone(),
            lease_id: lease.clone(),
            worker_id: None,
            attempt_id: None,
            epoch: None,
        };
        Ok(Self {
            _server: server,
            client,
            js,
            provider,
            lease,
            grant,
            payload,
            signer,
            scope,
            tasks: vec![run_task, cancel_task],
            active,
            anchors: Mutex::new(Vec::new()),
            worker,
            runtime_digest,
            runtime_volume,
            runtime_bytes,
        })
    }

    /// A detached, lease-bound, named launch of the native family for `marker` that requires launch cancellation.
    pub(super) fn request(&self, name: &str, key: &str, marker: &str) -> anyhow::Result<RunCommandRequest> {
        Ok(serde_json::from_value(json!({
            "lease_id": self.lease.as_str(),
            "command": ["node", "-e", FAMILY, marker],
            "env": { "SANDBOX_ACCEPTANCE_FAMILY": marker },
            "detached": true,
            "deadline": "lease",
            "process": { "name": name },
            "require_launch_cancellation": true,
            "launch_key": key,
        }))?)
    }

    pub(super) fn subject(&self, verb: &str) -> String {
        format!("sandbox.provider.local_docker.{verb}.{}", self.lease.as_str())
    }

    pub(super) async fn send<T: Serialize, R: serde::de::DeserializeOwned>(
        &self,
        verb: &str,
        request: &T,
        scope: &VerbScope,
    ) -> anyhow::Result<R> {
        let subject = self.subject(verb);
        let bytes = serde_json::to_vec_pretty(request)?;
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(AUTH_HEADER, self.signer.sign_verb(&subject, &bytes, scope)?);
        let response = self
            .client
            .send_request(
                subject,
                async_nats::Request::new()
                    .headers(headers)
                    .payload(bytes.into())
                    .timeout(Some(Duration::from_secs(25))),
            )
            .await?;
        Ok(serde_json::from_slice(&response.payload)?)
    }

    pub(super) async fn start(&self, request: &RunCommandRequest) -> anyhow::Result<RunCommandStartedResponse> {
        let response: RunCommandResponse = self.send("run_command", request, &self.scope).await?;
        match response {
            RunCommandResponse::Started(started) => Ok(started),
            other => anyhow::bail!("actual strong command did not start: {other:?}"),
        }
    }

    pub(super) async fn cancel(&self, key: &str) -> anyhow::Result<CancelCommandLaunchResponse> {
        self.send(
            "cancel_command_launch",
            &CancelCommandLaunchRequest {
                lease_id: self.lease.clone(),
                launch_key: LaunchKey::new(key)?,
            },
            &self.scope,
        )
        .await
    }

    /// The helper and root identities of every admitted native registration for this lease, after confirming the
    /// container still runs the exact immutable supervisor artifact.
    pub(super) async fn observer_anchors(&self) -> anyhow::Result<Value> {
        let docker = bollard::Docker::connect_with_defaults()?;
        let container_id = self.grant.sandbox_id.as_str();
        let inspected = docker.inspect_container(container_id, None).await?;
        ensure!(
            inspected.id.as_deref() == Some(container_id),
            "observer canonical container changed"
        );
        let mounts: Vec<_> = inspected
            .mounts
            .as_ref()
            .context("observer mounts unavailable")?
            .iter()
            .filter(|mount| mount.name.as_deref() == Some(self.runtime_volume.as_str()))
            .collect();
        ensure!(
            mounts.len() == 1
                && mounts[0].typ == Some(bollard::config::MountPointTypeEnum::VOLUME)
                && mounts[0].rw == Some(false),
            "observer immutable runtime mount changed"
        );
        let root = mounts[0]
            .destination
            .as_deref()
            .context("observer runtime destination unavailable")?;
        ensure!(
            root.strip_prefix("/.trogon-sandbox-run-").is_some_and(is_fresh_id),
            "observer runtime mountpoint is not independently owned"
        );
        let volume = docker.inspect_volume(&self.runtime_volume).await?;
        ensure!(
            volume.name == self.runtime_volume
                && volume.driver == "local"
                && volume.options.is_empty()
                && volume.labels.get(OWNER_LABEL) == Some(&self.worker)
                && volume.labels.get(RUNTIME_DIGEST_LABEL) == Some(&self.runtime_digest),
            "observer runtime volume provenance changed"
        );
        let mut stream = docker.download_from_container(
            container_id,
            Some(
                DownloadFromContainerOptionsBuilder::new()
                    .path(&format!("{root}/supervisor"))
                    .build(),
            ),
        );
        let mut archive = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            ensure!(
                archive.len() + chunk.len() <= self.runtime_bytes.len() + 65536,
                "observer artifact archive exceeds its bound"
            );
            archive.extend_from_slice(&chunk);
        }
        let mut archive = tar::Archive::new(std::io::Cursor::new(archive));
        let mut count = 0;
        for entry in archive.entries()? {
            let mut entry = entry?;
            ensure!(
                entry.path()?.as_ref() == Path::new("supervisor")
                    && entry.header().entry_type().is_file()
                    && entry.size() == u64::try_from(self.runtime_bytes.len())?
                    && entry.header().mode()? == 0o555
                    && entry.header().uid()? == 0
                    && entry.header().gid()? == 0,
                "observer executable metadata changed"
            );
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            ensure!(
                bytes == self.runtime_bytes,
                "observer executable differs from host artifact"
            );
            count += 1;
        }
        ensure!(count == 1, "observer executable is not unique");
        let custody = self.js.get_key_value(CUSTODY_BUCKET).await?;
        let mut keys = custody.keys().await?;
        let mut anchors = Vec::new();
        let mut count = 0;
        while let Some(key) = keys.next().await {
            count += 1;
            ensure!(count <= 4096, "observer custody census exceeds its bound");
            let bytes = custody
                .get(key?)
                .await?
                .context("observer custody record disappeared")?;
            ensure!(bytes.len() <= 8192, "observer custody record exceeds its bound");
            let record: Value = serde_json::from_slice(&bytes)?;
            let owner = &record["owner"];
            if owner["tenant"].as_str() != Some(self.payload.request.tenant.as_str())
                || owner["lease_id"].as_str() != Some(self.lease.as_str())
                || owner["container_id"].as_str() != Some(container_id)
            {
                continue;
            }
            ensure!(
                owner["worker_id"].as_str() == Some(self.worker.as_str())
                    && owner["target"] == serde_json::to_value(&self.payload.target)?
                    && owner["registration_id"].as_str().is_some_and(is_fresh_id)
                    && record["version"].as_u64() == Some(2),
                "observer custody admission changed"
            );
            let native = &record["native"];
            if native.is_null() {
                continue;
            }
            ensure!(
                native["protocol"].as_u64() == Some(u64::from(supervisor::VERSION))
                    && native["artifact_digest"].as_str() == Some(self.runtime_digest.as_str()),
                "observer native artifact binding changed"
            );
            let helper = native["helper_pid"].as_u64().context("observer helper PID missing")?;
            let root = native["root_pid"].as_u64().context("observer root PID missing")?;
            let helper_start = native["helper_start_time"]
                .as_u64()
                .context("observer helper start missing")?;
            let root_start = native["root_start_time"]
                .as_u64()
                .context("observer root start missing")?;
            ensure!(
                helper > 1
                    && root > 1
                    && helper != root
                    && helper <= u64::from(u32::MAX)
                    && root <= u64::from(u32::MAX)
                    && helper_start > 0
                    && root_start > 0
                    && native["process_group"].as_u64() == Some(root)
                    && native["session"].as_u64() == Some(root),
                "observer native kernel binding invalid"
            );
            anchors.push(json!({
                "helper": { "pid": helper, "start": helper_start.to_string() },
                "root": { "pid": root, "start": root_start.to_string() },
            }));
        }
        Ok(Value::Array(anchors))
    }

    /// One census of the native family for `marker`, tracking `pinned` and the observer anchors seen so far.
    pub(super) async fn native(&self, marker: &str, pinned: &[Identity]) -> anyhow::Result<Native> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        tokio::time::timeout_at(deadline, async {
            let mut anchors = self.anchors.lock().await;
            let mut candidates = self.observer_anchors().await?;
            for attempt in 0..3 {
                let output = self
                    .provider
                    .run_command_for_lease(
                        &self.lease,
                        vec![
                            "node".into(),
                            "-e".into(),
                            PROBE.into(),
                            marker.into(),
                            serde_json::to_string(pinned)?,
                            serde_json::to_string(&*anchors)?,
                            serde_json::to_string(&candidates)?,
                        ],
                        None,
                        &CommandEnv::default(),
                        5,
                        None,
                        StreamedOutput::Disabled,
                        DEFAULT_MAX_OUTPUT_BYTES,
                    )
                    .await?;
                if output.exit_code == 0 {
                    let state: Native = serde_json::from_slice(&output.stdout)?;
                    anchors.clone_from(&state.anchors);
                    return Ok(state);
                }
                let failure = anyhow::anyhow!(
                    "native family probe failed: exit={} timed_out={:?} truncated={:?} stdout_bytes={} stderr_bytes={} stderr_class={}",
                    output.exit_code,
                    output.timed_out,
                    output.truncated,
                    output.stdout.len(),
                    output.stderr.len(),
                    native_output_class(&output.stderr)
                );
                let Some(diagnostic) = census_permission_diagnostic(&output.stderr) else {
                    return Err(failure);
                };
                let fresh = self
                    .observer_anchors()
                    .await
                    .map_err(|_| failure.context("fresh full observer provenance snapshot unavailable"))?;
                let matching: Vec<Identity> = fresh
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|anchor| serde_json::from_value::<Identity>(anchor["helper"].clone()).ok())
                    .filter(|identity| {
                        valid_identity(identity)
                            && identity.pid == diagnostic.pid
                            && Some(identity.start.as_str()) == diagnostic.start.as_deref()
                    })
                    .take(8)
                    .collect();
                eprintln!(
                    "native census permission diagnostic: denied_pid={} denied_start={:?} anchor_class={:?} captured_retained={:?} captured_candidates={:?} captured_verified={:?} retained_count={} candidate_count={} fresh_matching_helpers={:?} attempt={}",
                    diagnostic.pid,
                    diagnostic.start,
                    diagnostic.anchor_class,
                    diagnostic.retained,
                    diagnostic.candidates,
                    diagnostic.verified,
                    diagnostic.retained_count,
                    diagnostic.candidate_count,
                    matching,
                    attempt
                );
                if attempt < 2 && !matching.is_empty() {
                    candidates = fresh;
                    continue;
                }
                return Err(anyhow::anyhow!("native family census could not admit an unreadable process").context(diagnostic));
            }
            anyhow::bail!("native observer retry bound exceeded")
        })
        .await
        .context("native observation exceeded its original total budget")?
    }

    /// Waits until the family for `marker` has its root and child up and recorded once.
    pub(super) async fn ready(&self, marker: &str) -> anyhow::Result<Native> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let state = self.native(marker, &[]).await.context("native_ready_stage")?;
            if state.records.len() == 2 && state.starts.len() == 1 {
                ensure!(
                    state.records.iter().all(|record| state.members.contains(record)),
                    "native root/child census control failed"
                );
                return Ok(state);
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "native family failed to become ready"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    pub(super) async fn absent(&self, marker: &str, before: &Native) -> anyhow::Result<()> {
        let state = self
            .native(marker, &before.members)
            .await
            .context("native_absence_stage")?;
        ensure!(
            state.members.is_empty() && state.tracked_alive == 0,
            "owned native family survived cancellation"
        );
        ensure!(state.starts == before.starts, "user effect was replayed");
        Ok(())
    }

    pub(super) async fn preserved(&self, marker: &str, before: &Native) -> anyhow::Result<()> {
        let mut state = self
            .native(marker, &before.members)
            .await
            .context("native_preservation_stage")?;
        let mut original = before.members.clone();
        original.sort();
        state.members.sort();
        ensure!(
            state.members == original && state.tracked_alive == original.len() && state.starts == before.starts,
            "reused or unrelated native family changed"
        );
        Ok(())
    }

    pub(super) async fn cleanup(&mut self) -> anyhow::Result<()> {
        let container_cleanup = cleanup_owned_container(&self.provider, &self.lease, &self.grant).await;
        let runtime_cleanup = self.provider.shutdown_command_runtime().await;
        eprintln!(
            "native runtime cleanup: runtime_removed={} fallback_used=false",
            runtime_cleanup.is_ok()
        );
        container_cleanup?;
        runtime_cleanup?;
        self.active.lock().await.remove(&self.lease);
        for task in &self.tasks {
            task.abort();
        }
        Ok(())
    }
}
