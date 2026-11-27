use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use anyhow::{Context, ensure};
use bollard::Docker;
use trogon_sandbox_contract::{
    CancelIntent, LeaseAttemptIdentity, NativeHandleRequest, NativeHandleResponse, ProviderCleanupRequest,
    ProviderCleanupResponse, ProviderDispatchPayload, ProviderDispatchReply, ProviderKind, ProviderLease,
    RunCommandRequest, RunCommandResponse, WorkerId,
};
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner, OP_LEASE_DISPATCH, VerbScope};

use crate::targeted_process_support::{IMAGE, Native, Nats};

pub struct Worker(Option<Child>);

impl Worker {
    pub fn start(
        nats: &Nats,
        state: &Path,
        owner: &str,
        signer: &DispatchSigner,
        registry: &str,
    ) -> anyhow::Result<Self> {
        std::fs::create_dir(state)?;
        std::fs::set_permissions(state, std::fs::Permissions::from_mode(0o700))?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-docker-worker"));
        command.env_clear();
        for name in [
            "PATH",
            "HOME",
            "TMPDIR",
            "DOCKER_HOST",
            "DOCKER_CONTEXT",
            "DOCKER_CONFIG",
            "DOCKER_CERT_PATH",
            "DOCKER_TLS_VERIFY",
            "XDG_RUNTIME_DIR",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
        ] {
            if let Some(value) = trogon_std::env::ReadEnv::var_os(&trogon_std::env::SystemEnv, name) {
                command.env(name, value);
            }
        }
        let child = command
            .env("SANDBOX_NATS_URL", &nats.url)
            .env("SANDBOX_DOCKER_WORKER_OWNER", owner)
            .env("SANDBOX_DOCKER_STATE_DIR", state)
            .env("SANDBOX_IMAGE", IMAGE)
            .env("SANDBOX_EGRESS_PROXY_IMAGE", IMAGE)
            .env("SANDBOX_SNAPSHOT_REGISTRY", registry)
            .env("SANDBOX_OCI_INSECURE_REGISTRIES", registry)
            .env("SANDBOX_WARM_POOL_ENABLED", "true")
            .env("SANDBOX_WARM_POOL_MAX_PER_DIGEST", "1")
            .env("SANDBOX_WARM_POOL_MAX_TOTAL", "1")
            .env("SANDBOX_WARM_POOL_MAX_IDLE_SECONDS", "600")
            .env("SANDBOX_METRICS_ADDR", "127.0.0.1:0")
            .env("SANDBOX_WORKER_AUTHORITY", "off")
            .env("SANDBOX_DISPATCH_VERIFY", "require")
            .env("SANDBOX_FENCE_EPOCH", "require")
            .env("SANDBOX_DISPATCH_FENCE_FILE", state.join("dispatch-fence.jsonl"))
            .env(
                "SANDBOX_DISPATCH_PUBLIC_KEYS",
                format!("{}:{}", signer.key_id(), signer.public_key_b64()),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("actual warm-assignment worker startup failed")?;
        Ok(Self(Some(child)))
    }

    pub fn alive(&mut self) -> anyhow::Result<bool> {
        Ok(self
            .0
            .as_mut()
            .context("owned warm worker stopped")?
            .try_wait()?
            .is_none())
    }

    pub fn stop(&mut self) -> anyhow::Result<()> {
        if let Some(mut child) = self.0.take() {
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            child.wait()?;
        }
        Ok(())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub async fn dispatch(
    nats: &Nats,
    signer: &DispatchSigner,
    subject: &str,
    payload: &ProviderDispatchPayload,
) -> anyhow::Result<ProviderDispatchReply> {
    let body = signer
        .sign_payload(&serde_json::to_vec(payload)?, OP_LEASE_DISPATCH)
        .map_err(anyhow::Error::msg)?;
    let reply = nats
        .client
        .send_request(
            subject.to_owned(),
            async_nats::Request::new()
                .payload(body.into())
                .timeout(Some(Duration::from_secs(30))),
        )
        .await?;
    Ok(serde_json::from_slice(&reply.payload)?)
}

pub fn granted(reply: ProviderDispatchReply) -> anyhow::Result<ProviderLease> {
    match reply {
        ProviderDispatchReply::Granted(receipt) => Ok(receipt),
        _ => anyhow::bail!("actual warm fixture positive allocation was refused"),
    }
}

fn headers(
    signer: &DispatchSigner,
    subject: &str,
    bytes: &[u8],
    assigned: &ProviderDispatchPayload,
) -> anyhow::Result<async_nats::HeaderMap> {
    let scope = VerbScope {
        tenant: assigned.request.tenant.clone(),
        lease_id: assigned.lease_id.clone(),
        worker_id: assigned.target.as_ref().map(|target| target.worker_id.clone()),
        attempt_id: assigned.target.as_ref().map(|target| target.attempt_id.clone()),
        epoch: assigned.target.as_ref().and_then(|target| target.epoch),
    };
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(AUTH_HEADER, signer.sign_verb(subject, bytes, &scope)?);
    Ok(headers)
}

async fn rpc<T: serde::Serialize, R: serde::de::DeserializeOwned>(
    nats: &Nats,
    signer: &DispatchSigner,
    assigned: &ProviderDispatchPayload,
    subject: String,
    request: &T,
) -> anyhow::Result<R> {
    let bytes = serde_json::to_vec(request)?;
    let reply = nats
        .client
        .send_request(
            subject.clone(),
            async_nats::Request::new()
                .headers(headers(signer, &subject, &bytes, assigned)?)
                .payload(bytes.into())
                .timeout(Some(Duration::from_secs(15))),
        )
        .await?;
    Ok(serde_json::from_slice(&reply.payload)?)
}

pub async fn command(
    nats: &Nats,
    signer: &DispatchSigner,
    assigned: &ProviderDispatchPayload,
    script: &str,
    expected: &[u8],
) -> anyhow::Result<()> {
    let request: RunCommandRequest = serde_json::from_value(serde_json::json!({
        "lease_id": assigned.lease_id, "command": ["node", "-e", script], "timeout_seconds": 5, "max_output_bytes": 4096
    }))?;
    let reply: RunCommandResponse = rpc(
        nats,
        signer,
        assigned,
        trogon_sandbox_protocol::subjects::provider_run_command(ProviderKind::LocalDocker, &assigned.lease_id),
        &request,
    )
    .await?;
    ensure!(
        matches!(reply, RunCommandResponse::Completed(ref output)
        if output.exit_code == 0 && output.stdout == expected && output.stderr.is_empty() && output.timed_out.is_within_deadline() && output.truncated.is_complete()),
        "warm assignment exact ordinary command control failed"
    );
    Ok(())
}

#[derive(Clone, Copy)]
pub enum OldCommandOutcome {
    Denied,
    SilentNonholder,
    Executed,
}

impl OldCommandOutcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::SilentNonholder => "bounded_no_reply",
            Self::Executed => "executed",
        }
    }

    pub fn did_not_acknowledge_execution(self) -> bool {
        !matches!(self, Self::Executed)
    }
}

pub async fn old_command_probe(
    nats: &Nats,
    signer: &DispatchSigner,
    prior: &ProviderDispatchPayload,
    marker: &str,
) -> anyhow::Result<OldCommandOutcome> {
    let request: RunCommandRequest = serde_json::from_value(serde_json::json!({
        "lease_id": prior.lease_id,
        "command": ["node", "-e", format!("require('fs').writeFileSync({marker:?},'forbidden-old-owner')")],
        "timeout_seconds": 5, "max_output_bytes": 4096
    }))?;
    let subject = trogon_sandbox_protocol::subjects::provider_run_command(ProviderKind::LocalDocker, &prior.lease_id);
    let bytes = serde_json::to_vec(&request)?;
    match nats
        .client
        .send_request(
            subject.clone(),
            async_nats::Request::new()
                .headers(headers(signer, &subject, &bytes, prior)?)
                .payload(bytes.into())
                .timeout(Some(Duration::from_secs(5))),
        )
        .await
    {
        Ok(message) => Ok(match serde_json::from_slice::<RunCommandResponse>(&message.payload)? {
            RunCommandResponse::Denied(_) => OldCommandOutcome::Denied,
            RunCommandResponse::Completed(_) | RunCommandResponse::Started(_) => OldCommandOutcome::Executed,
        }),
        Err(error) if error.kind() == async_nats::RequestErrorKind::TimedOut => Ok(OldCommandOutcome::SilentNonholder),
        Err(error) => Err(error.into()),
    }
}

pub async fn legacy_release(
    nats: &Nats,
    signer: &DispatchSigner,
    assigned: &ProviderDispatchPayload,
) -> anyhow::Result<()> {
    let subject = trogon_sandbox_protocol::subjects::provider_cancel(ProviderKind::LocalDocker, &assigned.lease_id);
    nats.client
        .publish_with_headers(
            subject.clone(),
            headers(signer, &subject, &[], assigned)?,
            Vec::new().into(),
        )
        .await?;
    nats.client.flush().await?;
    Ok(())
}

pub async fn cleanup(
    nats: &Nats,
    signer: &DispatchSigner,
    assigned: &ProviderDispatchPayload,
    worker: &WorkerId,
) -> anyhow::Result<ProviderCleanupResponse> {
    let target = assigned
        .target
        .clone()
        .context("warm assigned cleanup requires exact target")?;
    rpc(
        nats,
        signer,
        assigned,
        trogon_sandbox_protocol::subjects::worker_cleanup(ProviderKind::LocalDocker, worker),
        &ProviderCleanupRequest {
            identity: LeaseAttemptIdentity {
                lease_id: assigned.lease_id.clone(),
                provider: ProviderKind::LocalDocker,
                target,
            },
            intent: CancelIntent::Release,
        },
    )
    .await
}

pub fn exact_cleanup(response: &ProviderCleanupResponse, assigned: &ProviderDispatchPayload) -> bool {
    matches!(response, ProviderCleanupResponse::Cleaned { identity, intent: CancelIntent::Release }
        if identity.lease_id == assigned.lease_id && identity.provider == ProviderKind::LocalDocker && Some(&identity.target) == assigned.target.as_ref())
}

pub async fn native_handle(
    nats: &Nats,
    signer: &DispatchSigner,
    assigned: &ProviderDispatchPayload,
) -> anyhow::Result<NativeHandleResponse> {
    rpc(
        nats,
        signer,
        assigned,
        trogon_sandbox_protocol::subjects::provider_native_handle(ProviderKind::LocalDocker, &assigned.lease_id),
        &NativeHandleRequest {
            lease_id: assigned.lease_id.clone(),
        },
    )
    .await
}

pub struct BirthPin {
    pub id: String,
    pub image: String,
    lease: String,
    init_pid: i64,
    started_at: String,
}

impl BirthPin {
    pub async fn seal(
        docker: &Docker,
        native: &Native,
        receipt: &ProviderLease,
        birth_lease: &str,
    ) -> anyhow::Result<Self> {
        native.running(receipt.sandbox_id.as_str(), birth_lease).await?;
        let inspected = docker.inspect_container(receipt.sandbox_id.as_str(), None).await?;
        let state = inspected.state.context("warm source init state absent")?;
        ensure!(
            inspected
                .config
                .as_ref()
                .and_then(|config| config.labels.as_ref())
                .is_some_and(|labels| labels
                    .get("sandbox.trogon.live/role")
                    .is_some_and(|role| role == "workload")),
            "warm source birth role is not an owned workload"
        );
        Ok(Self {
            id: receipt.sandbox_id.as_str().to_owned(),
            image: inspected.image.context("warm source immutable image absent")?,
            lease: birth_lease.into(),
            init_pid: state
                .pid
                .filter(|pid| *pid > 0)
                .context("warm source init PID absent")?,
            started_at: state.started_at.context("warm source StartedAt absent")?.to_string(),
        })
    }

    pub async fn verify(&self, docker: &Docker, native: &Native) -> anyhow::Result<()> {
        native.running(&self.id, &self.lease).await?;
        let inspected = docker.inspect_container(&self.id, None).await?;
        let state = inspected.state.context("pooled native state absent")?;
        ensure!(
            inspected.image.as_ref() == Some(&self.image)
                && state.pid == Some(self.init_pid)
                && state.started_at.as_ref().map(ToString::to_string).as_deref() == Some(self.started_at.as_str()),
            "exact pooled CID, immutable image, init PID or StartedAt changed"
        );
        Ok(())
    }
}
