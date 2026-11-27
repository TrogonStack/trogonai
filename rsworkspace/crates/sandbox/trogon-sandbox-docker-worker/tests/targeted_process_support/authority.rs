use super::*;
use std::os::unix::fs::MetadataExt;

pub(super) struct CanonicalFenceFault {
    pub(super) canonical: PathBuf,
    pub(super) original: PathBuf,
    pub(super) original_inode: u64,
    pub(super) original_device: u64,
    pub(super) original_prefix: Vec<u8>,
}

impl CanonicalFenceFault {
    pub(super) fn replace(canonical: &Path) -> anyhow::Result<Self> {
        let original_prefix = std::fs::read(canonical)?;
        ensure!(
            !original_prefix.is_empty(),
            "positive assignments must retain durable fence authority"
        );
        let metadata = std::fs::metadata(canonical)?;
        let original_inode = metadata.ino();
        let original_device = metadata.dev();
        let original = canonical.with_extension("cleanup-retained-original");
        std::fs::rename(canonical, &original)?;
        let fault = Self {
            canonical: canonical.into(),
            original,
            original_inode,
            original_device,
            original_prefix,
        };
        std::fs::write(&fault.canonical, [])?;
        ensure!(
            fault.present()?,
            "canonical dispatch fence identity fault was not installed"
        );
        Ok(fault)
    }

    pub(super) fn present(&self) -> anyhow::Result<bool> {
        let original = std::fs::metadata(&self.original)?;
        let canonical = std::fs::metadata(&self.canonical)?;
        // Cleanup admission may append its tombstone to the already-open original.
        Ok(original.ino() == self.original_inode
            && original.dev() == self.original_device
            && canonical.ino() != self.original_inode
            && canonical.dev() == self.original_device
            && canonical.len() == 0
            && std::fs::read(&self.original)?.starts_with(&self.original_prefix))
    }

    pub(super) fn retained_bytes(&self) -> anyhow::Result<Vec<u8>> {
        Ok(std::fs::read(&self.original)?)
    }

    pub(super) fn restore_for_physical_cleanup(&mut self) -> anyhow::Result<()> {
        if self.original.exists() {
            std::fs::remove_file(&self.canonical)?;
            std::fs::rename(&self.original, &self.canonical)?;
        }
        Ok(())
    }
}

impl Drop for CanonicalFenceFault {
    fn drop(&mut self) {
        let _ = self.restore_for_physical_cleanup();
    }
}

pub(super) struct NativePin {
    pub(super) id: String,
    pub(super) image: String,
    pub(super) init_pid: i64,
    pub(super) started_at: String,
}

impl NativePin {
    pub(super) async fn seal(
        native: &Native,
        docker: &bollard::Docker,
        grant: &ProviderLease,
        lease: &LeaseId,
    ) -> anyhow::Result<Self> {
        native.running(grant.sandbox_id.as_str(), lease.as_str()).await?;
        let found = docker.inspect_container(grant.sandbox_id.as_str(), None).await?;
        let state = found.state.context("positive native init state is absent")?;
        Ok(Self {
            id: grant.sandbox_id.as_str().to_owned(),
            image: found.image.context("positive native image identity is absent")?,
            init_pid: state
                .pid
                .filter(|pid| *pid > 0)
                .context("positive native init PID is absent")?,
            started_at: state
                .started_at
                .context("positive native init StartedAt is absent")?
                .to_string(),
        })
    }

    pub(super) async fn verify(
        &self,
        native: &Native,
        docker: &bollard::Docker,
        lease: &LeaseId,
    ) -> anyhow::Result<()> {
        native.running(&self.id, lease.as_str()).await?;
        let found = docker.inspect_container(&self.id, None).await?;
        let state = found.state.context("retained native init state is absent")?;
        ensure!(
            found.image.as_deref() == Some(self.image.as_str())
                && state.pid == Some(self.init_pid)
                && state.started_at.as_ref().map(ToString::to_string).as_deref() == Some(self.started_at.as_str()),
            "retained native image, init PID or StartedAt changed"
        );
        Ok(())
    }
}

pub(super) fn exact_cleanup(response: &ProviderCleanupResponse, assigned: &ProviderDispatchPayload) -> bool {
    matches!(response, ProviderCleanupResponse::Cleaned { identity, intent: CancelIntent::Release }
        if identity.lease_id == assigned.lease_id && identity.provider == ProviderKind::LocalDocker
            && Some(&identity.target) == assigned.target.as_ref())
}

pub(super) fn cleanup_refused(response: Value) -> bool {
    match serde_json::from_value::<ProviderCleanupResponse>(response.clone()) {
        Ok(ProviderCleanupResponse::Failed { .. }) => true,
        Ok(ProviderCleanupResponse::Cleaned { .. }) => false,
        Err(_) => response.get("error").and_then(Value::as_str) == Some("dispatch_auth"),
    }
}

pub(super) fn exact_cleanup_tombstone(before: &[u8], after: &[u8], assigned: &ProviderDispatchPayload) -> bool {
    let (Some(target), Some(appended)) = (assigned.target.as_ref(), after.strip_prefix(before)) else {
        return false;
    };
    let lines: Vec<_> = appended
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect();
    let [line] = lines.as_slice() else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<Value>(line) else {
        return false;
    };
    value.as_object().is_some_and(|object| object.len() == 7)
        && value["l"].as_str() == Some(assigned.lease_id.as_str())
        && value["t"].as_str() == Some(assigned.request.tenant.as_str())
        && value["w"].as_str() == Some(target.worker_id.as_str())
        && value["a"].as_str() == Some(target.attempt_id.as_str())
        && value["e"].as_u64() == target.epoch.map(LeaseEpoch::get)
        && value["c"] == true
        && value["x"] == false
}

pub(super) async fn ordinary_cleanup_control(
    nats: &Nats,
    signer: &DispatchSigner,
    assigned: &ProviderDispatchPayload,
) -> anyhow::Result<()> {
    let request: trogon_sandbox_contract::RunCommandRequest = serde_json::from_value(serde_json::json!({
        "lease_id": assigned.lease_id, "command": ["node", "-e", "process.stdout.write('lease-cleanup-control')"],
        "timeout_seconds": 5, "max_output_bytes": 4096
    }))?;
    let subject =
        trogon_sandbox_protocol::subjects::provider_run_command(ProviderKind::LocalDocker, &assigned.lease_id);
    let bytes = serde_json::to_vec(&request)?;
    let message = nats
        .client
        .send_request(
            subject.clone(),
            async_nats::Request::new()
                .headers(headers(signer, &subject, &bytes, assigned)?)
                .payload(bytes.into())
                .timeout(Some(Duration::from_secs(10))),
        )
        .await?;
    let response: trogon_sandbox_contract::RunCommandResponse = serde_json::from_slice(&message.payload)?;
    ensure!(
        matches!(response, trogon_sandbox_contract::RunCommandResponse::Completed(ref output)
        if output.exit_code == 0 && output.stdout == b"lease-cleanup-control" && output.stderr.is_empty()
            && output.timed_out.is_within_deadline() && output.truncated.is_complete()),
        "healthy assigned ordinary native control failed"
    );
    Ok(())
}

pub(super) async fn rollback_rpc<T: serde::Serialize, R: serde::de::DeserializeOwned>(
    nats: &Nats,
    signer: &DispatchSigner,
    assigned: &ProviderDispatchPayload,
    subject: String,
    request: &T,
) -> anyhow::Result<R> {
    let bytes = serde_json::to_vec(request)?;
    let message = nats
        .client
        .send_request(
            subject.clone(),
            async_nats::Request::new()
                .headers(headers(signer, &subject, &bytes, assigned)?)
                .payload(bytes.into())
                .timeout(Some(Duration::from_secs(30))),
        )
        .await?;
    Ok(serde_json::from_slice(&message.payload)?)
}

pub(super) async fn rollback_native_handle(
    nats: &Nats,
    signer: &DispatchSigner,
    assigned: &ProviderDispatchPayload,
) -> anyhow::Result<String> {
    let response: trogon_sandbox_contract::NativeHandleResponse = rollback_rpc(
        nats,
        signer,
        assigned,
        trogon_sandbox_protocol::subjects::provider_native_handle(ProviderKind::LocalDocker, &assigned.lease_id),
        &trogon_sandbox_contract::NativeHandleRequest {
            lease_id: assigned.lease_id.clone(),
        },
    )
    .await?;
    let trogon_sandbox_contract::NativeHandleResponse::Available(handle) = response else {
        anyhow::bail!("public rollback native-handle control is unavailable");
    };
    ensure!(
        handle.provider == ProviderKind::LocalDocker
            && handle.vendor_id.len() == 64
            && handle.vendor_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "public native handle is not an exact canonical Docker ID"
    );
    Ok(handle.vendor_id)
}

pub(super) async fn snapshot_command_control(
    nats: &Nats,
    signer: &DispatchSigner,
    assigned: &ProviderDispatchPayload,
    marker: &str,
) -> anyhow::Result<()> {
    let script = format!(
        "if(require('fs').readFileSync('/tmp/rollback-snapshot-control','utf8')!=={marker:?})process.exit(7);process.stdout.write('valid-snapshot-control')"
    );
    let request: trogon_sandbox_contract::RunCommandRequest = serde_json::from_value(serde_json::json!({
        "lease_id": assigned.lease_id, "command": ["node", "-e", script],
        "timeout_seconds": 5, "max_output_bytes": 4096
    }))?;
    let response: trogon_sandbox_contract::RunCommandResponse = rollback_rpc(
        nats,
        signer,
        assigned,
        trogon_sandbox_protocol::subjects::provider_run_command(ProviderKind::LocalDocker, &assigned.lease_id),
        &request,
    )
    .await?;
    ensure!(
        matches!(response, trogon_sandbox_contract::RunCommandResponse::Completed(ref output)
        if output.exit_code == 0 && output.stdout == b"valid-snapshot-control" && output.stderr.is_empty()
            && output.timed_out.is_within_deadline() && output.truncated.is_complete()),
        "valid snapshot ordinary native command control failed"
    );
    Ok(())
}

pub(super) async fn rollback_effect_barrier(
    nats: &Nats,
    signer: &DispatchSigner,
    sentinel: &ProviderDispatchPayload,
    native_id: &str,
    effects: &rollback_snapshot::NativeEffects,
) -> anyhow::Result<()> {
    let marker = format!("rollback-effect-barrier-{}", fresh_id());
    effects.register_barrier(&marker, native_id)?;
    let request: trogon_sandbox_contract::RunCommandRequest = serde_json::from_value(serde_json::json!({
        "lease_id": sentinel.lease_id,
        "command": ["node", "-e", format!("process.stdout.write('{marker}')")],
        "timeout_seconds": 5, "max_output_bytes": 4096
    }))?;
    let response: trogon_sandbox_contract::RunCommandResponse = rollback_rpc(
        nats,
        signer,
        sentinel,
        trogon_sandbox_protocol::subjects::provider_run_command(ProviderKind::LocalDocker, &sentinel.lease_id),
        &request,
    )
    .await?;
    ensure!(
        matches!(response, trogon_sandbox_contract::RunCommandResponse::Completed(ref output)
        if output.exit_code == 0 && output.stdout == marker.as_bytes() && output.stderr.is_empty()
            && output.timed_out.is_within_deadline() && output.truncated.is_complete()),
        "owned effect barrier ordinary command failed"
    );
    effects.wait_for_barrier(&marker).await
}
