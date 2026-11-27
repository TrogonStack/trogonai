#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::time::Duration;

use anyhow::{Context, ensure};
use bollard::Docker;
use futures::{FutureExt, StreamExt};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    BootMode, CancelIntent, CommandOutput, ContainerOverrides, DispatchAttemptId, DispatchTarget, ExtraCapability,
    LeaseAttemptIdentity, LeaseEpoch, LeaseId, LeaseRequest, LeaseRequestKey, NetworkPolicy, PlatformOptions,
    PollCommandRequest, PollCommandResponse, PrivilegedAccess, ProviderCleanupRequest, ProviderCleanupResponse,
    ProviderDispatchPayload, ProviderDispatchReply, ProviderKind, ProviderLease, RunCommandRequest, RunCommandResponse,
    SandboxSpec, SandboxSpecParts, SandboxTags, SandboxUserName, TenantId, WorkerDispatchProtocol, WorkerHeartbeat,
    WorkerId, WorkspaceSource,
};
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner, OP_LEASE_DISPATCH, VerbScope, subjects};

mod command_readiness_support;
mod targeted_process_support;

use command_readiness_support::{
    Binding, ExistingScopeRefusalReason, Response, Runtime, Supervisor, UnsupportedReason,
    assert_existing_scope_refusal,
};
use targeted_process_support::{IMAGE, Native, Nats, fresh_id};

const WAIT: Duration = Duration::from_secs(15);
const ROOT_CONTROL: &str = r#"const fs=require('node:fs');const s=fs.readFileSync('/proc/self/status','utf8');const c=s.match(/^CapEff:\s*([0-9a-f]+)$/m);if(!c)throw Error('capability status absent');process.stdout.write(JSON.stringify({uid:process.getuid(),ptrace:(BigInt('0x'+c[1])&(1n<<19n))!==0n}));"#;
const EFFECT: &str = "/tmp/readiness-forbidden-effect";
const TENANT: &str = "readiness-tenant";
const FOREIGN_TENANT: &str = "foreign-readiness-tenant";

#[derive(Clone)]
struct Lease {
    dispatch: ProviderDispatchPayload,
    grant: ProviderLease,
}

impl Lease {
    fn target(&self) -> anyhow::Result<&DispatchTarget> {
        self.dispatch
            .target
            .as_ref()
            .context("readiness lease has no assigned target")
    }

    fn binding(&self) -> anyhow::Result<Binding> {
        Ok(Binding {
            tenant: self.dispatch.request.tenant.clone(),
            lease_id: self.dispatch.lease_id.clone(),
            provider: ProviderKind::LocalDocker,
            target: self.target()?.clone(),
            sandbox_id: self.grant.sandbox_id.as_str().to_owned(),
            run_as_user: Some(SandboxUserName::new("root")?),
        })
    }
}

/// The verb scopes the readiness lane sends with a signed request.
#[derive(Clone, Copy)]
enum ScopeVariant {
    Exact,
    ForeignTenant,
    UnknownAttempt,
}

/// The single binding field each foreign readiness request changes.
#[derive(Clone, Copy, Debug)]
enum ForeignField {
    Tenant,
    Provider,
    Worker,
    Attempt,
    Epoch,
    Native,
}

fn scope(dispatch: &ProviderDispatchPayload, variant: ScopeVariant) -> anyhow::Result<VerbScope> {
    let target = dispatch.target.as_ref().context("readiness dispatch has no target")?;
    let mut scope = VerbScope {
        tenant: dispatch.request.tenant.clone(),
        lease_id: dispatch.lease_id.clone(),
        worker_id: Some(target.worker_id.clone()),
        attempt_id: Some(target.attempt_id.clone()),
        epoch: target.epoch,
    };
    match variant {
        ScopeVariant::Exact => {}
        ScopeVariant::ForeignTenant => scope.tenant = TenantId::new(FOREIGN_TENANT)?,
        ScopeVariant::UnknownAttempt => scope.attempt_id = Some(DispatchAttemptId::new("unknown-readiness-attempt")?),
    }
    Ok(scope)
}

fn subject(lease: &Lease, verb: &str) -> String {
    format!(
        "sandbox.provider.{}.{verb}.{}",
        subjects::provider_token(ProviderKind::LocalDocker),
        lease.dispatch.lease_id.as_str()
    )
}

fn readiness_subject(lease: &Lease) -> anyhow::Result<String> {
    Ok(subjects::worker_command_readiness(
        ProviderKind::LocalDocker,
        &lease.target()?.worker_id,
        &lease.dispatch.lease_id,
    ))
}

async fn raw(
    nats: &Nats,
    signer: &DispatchSigner,
    subject: &str,
    body: Vec<u8>,
    scope: &VerbScope,
    timeout: Duration,
) -> anyhow::Result<Result<async_nats::Message, async_nats::RequestError>> {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(AUTH_HEADER, signer.sign_verb(subject, &body, scope)?);
    Ok(nats
        .client
        .send_request(
            subject.to_owned(),
            async_nats::Request::new()
                .headers(headers)
                .payload(body.into())
                .timeout(Some(timeout)),
        )
        .await)
}

async fn rpc<T: Serialize, R: DeserializeOwned>(
    nats: &Nats,
    signer: &DispatchSigner,
    lease: &Lease,
    verb: &str,
    request: &T,
) -> anyhow::Result<R> {
    let message = raw(
        nats,
        signer,
        &subject(lease, verb),
        serde_json::to_vec(request)?,
        &scope(&lease.dispatch, ScopeVariant::Exact)?,
        WAIT,
    )
    .await??;
    serde_json::from_slice(&message.payload).context("actual worker reply violated its public contract")
}

fn dispatch(
    worker: &WorkerId,
    name: &str,
    ptrace: bool,
    privileged: PrivilegedAccess,
) -> anyhow::Result<ProviderDispatchPayload> {
    let lease = LeaseId::new(format!("readiness-{name}"));
    let mut platform = PlatformOptions::default();
    if ptrace {
        platform.container = ContainerOverrides {
            extra_capabilities: vec![ExtraCapability::new("SYS_PTRACE")?],
            ..ContainerOverrides::default()
        };
    }
    let spec = SandboxSpec::new(SandboxSpecParts {
        image: targeted_process_support::registry_image(),
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::MIN,
        memory_mib: NonZeroU32::new(256).context("memory")?,
        snapshot_out: None,
        network_policy: Some(NetworkPolicy::DenyAll),
        mounts: vec![],
        drives: vec![],
        exposed_ports: vec![],
        boot_mode: BootMode::Bare,
        working_dir: Some("/tmp".into()),
        user: Some("0".into()),
        privileged,
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform,
        idle_timeout_seconds: None,
        no_ttl: Default::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: Default::default(),
    })?;
    Ok(ProviderDispatchPayload {
        lease_id: lease.clone(),
        request: LeaseRequest {
            operation_id: None,
            tenant: TenantId::new(TENANT)?,
            provider_pref: vec![ProviderKind::LocalDocker],
            spec,
            ttl_seconds: 120,
            idempotency_key: LeaseRequestKey::new(lease.as_str())?,
            isolation_tier: None,
            host_imports: vec![],
            required_capabilities: None,
            prewarm: Default::default(),
            tags: SandboxTags::default(),
            name: None,
            persistent: Default::default(),
            native: None,
            region: None,
            fallback_regions: vec![],
        },
        correlation_id: None,
        resolved_drives: vec![],
        target: Some(DispatchTarget {
            worker_id: worker.clone(),
            attempt_id: DispatchAttemptId::new(format!("{name}-attempt"))?,
            epoch: Some(LeaseEpoch::FIRST),
        }),
    })
}

async fn send_dispatch(
    nats: &Nats,
    signer: &DispatchSigner,
    dispatch: &ProviderDispatchPayload,
) -> anyhow::Result<ProviderDispatchReply> {
    let target = dispatch.target.as_ref().context("readiness dispatch has no target")?;
    let signed = signer.sign_payload(&serde_json::to_vec(dispatch)?, OP_LEASE_DISPATCH)?;
    let message = nats
        .client
        .send_request(
            subjects::worker_dispatch(ProviderKind::LocalDocker, &target.worker_id),
            async_nats::Request::new().payload(signed.into()).timeout(Some(WAIT)),
        )
        .await?;
    Ok(serde_json::from_slice(&message.payload)?)
}

async fn allocate(
    nats: &Nats,
    signer: &DispatchSigner,
    native: &mut Native,
    dispatch: ProviderDispatchPayload,
) -> anyhow::Result<Lease> {
    native.register_lease(dispatch.lease_id.as_str());
    let reply = send_dispatch(nats, signer, &dispatch).await?;
    let ProviderDispatchReply::Granted(grant) = reply else {
        anyhow::bail!("actual signed targeted allocation refused its native control")
    };
    native.register_id(grant.sandbox_id.as_str());
    let target = dispatch.target.as_ref().context("readiness dispatch has no target")?;
    ensure!(
        grant.worker_id.as_ref() == Some(&target.worker_id)
            && grant.attempt_id.as_ref() == Some(&target.attempt_id)
            && grant.epoch == target.epoch,
        "native receipt changed its actual assigned target"
    );
    native
        .running(grant.sandbox_id.as_str(), dispatch.lease_id.as_str())
        .await?;
    Ok(Lease { dispatch, grant })
}

fn command(lease: &Lease, code: &str, strong: bool, key: &str) -> anyhow::Result<RunCommandRequest> {
    Ok(serde_json::from_value(json!({
        "lease_id":lease.dispatch.lease_id, "command":["node","-e",code],
        "cwd":"/tmp", "run_as_user":"root", "detached":true, "stdin":"closed",
        "launch_key":if strong { Some(key) } else { None }, "require_launch_cancellation":strong,
        "timeout_seconds":10,
    }))?)
}

async fn completed(
    nats: &Nats,
    signer: &DispatchSigner,
    lease: &Lease,
    request: &RunCommandRequest,
) -> anyhow::Result<CommandOutput> {
    let started: RunCommandResponse = rpc(nats, signer, lease, "run_command", request).await?;
    let RunCommandResponse::Started(started) = started else {
        anyhow::bail!("healthy actual native command did not start: {started:?}")
    };
    tokio::time::timeout(WAIT, async {
        loop {
            let polled: PollCommandResponse = rpc(
                nats,
                signer,
                lease,
                "poll_command",
                &PollCommandRequest {
                    lease_id: lease.dispatch.lease_id.clone(),
                    execution_id: started.execution_id.clone(),
                },
            )
            .await?;
            match polled {
                PollCommandResponse::Running => tokio::time::sleep(Duration::from_millis(10)).await,
                PollCommandResponse::Completed(output) => {
                    ensure!(
                        output.exit_code == 0
                            && output.stderr.is_empty()
                            && output.truncated.is_complete()
                            && output.timed_out.is_within_deadline(),
                        "actual native control did not complete cleanly"
                    );
                    return Ok(output);
                }
                _ => anyhow::bail!("actual native command lost its exact execution receipt"),
            }
        }
    })
    .await
    .context("actual native control completion deadline")?
}

async fn no_effect(nats: &Nats, signer: &DispatchSigner, lease: &Lease) -> anyhow::Result<()> {
    let code = format!(
        "process.stdout.write(String(require('node:fs').existsSync({})))",
        serde_json::to_string(EFFECT)?
    );
    let output = completed(nats, signer, lease, &command(lease, &code, false, "")?).await?;
    ensure!(
        output.stdout == b"false",
        "a refused readiness or scope request produced a user effect"
    );
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(tag = "transport", rename_all = "snake_case")]
enum Observation {
    Reply { response: Response },
    NoResponders,
    Timeout,
}

async fn readiness(
    nats: &Nats,
    signer: &DispatchSigner,
    lease: &Lease,
    binding: &Binding,
) -> anyhow::Result<Observation> {
    match raw(
        nats,
        signer,
        &readiness_subject(lease)?,
        serde_json::to_vec(binding)?,
        &scope(&lease.dispatch, ScopeVariant::Exact)?,
        WAIT,
    )
    .await?
    {
        Ok(message) => Ok(Observation::Reply {
            response: serde_json::from_slice(&message.payload)
                .context("readiness RPC did not return its strict typed response")?,
        }),
        Err(error) if error.kind() == async_nats::RequestErrorKind::NoResponders => Ok(Observation::NoResponders),
        Err(error) if error.kind() == async_nats::RequestErrorKind::TimedOut => Ok(Observation::Timeout),
        Err(error) => Err(error.into()),
    }
}

fn foreign_binding(safe: &Binding, field: ForeignField) -> anyhow::Result<Binding> {
    let mut foreign = safe.clone();
    match field {
        ForeignField::Tenant => foreign.tenant = TenantId::new(FOREIGN_TENANT)?,
        ForeignField::Provider => foreign.provider = ProviderKind::E2b,
        ForeignField::Worker => foreign.target.worker_id = WorkerId::new("foreign-worker")?,
        ForeignField::Attempt => foreign.target.attempt_id = DispatchAttemptId::new("foreign-attempt")?,
        ForeignField::Epoch => foreign.target.epoch = Some(LeaseEpoch::new(2)?),
        ForeignField::Native => foreign.sandbox_id = "0".repeat(64),
    }
    Ok(foreign)
}

async fn cleanup(nats: &Nats, signer: &DispatchSigner, lease: &Lease, native: &Native) -> anyhow::Result<()> {
    let target = lease.target()?;
    let request = ProviderCleanupRequest {
        identity: LeaseAttemptIdentity {
            lease_id: lease.dispatch.lease_id.clone(),
            provider: ProviderKind::LocalDocker,
            target: target.clone(),
        },
        intent: CancelIntent::Release,
    };
    let route = subjects::worker_cleanup(ProviderKind::LocalDocker, &target.worker_id);
    let response = raw(
        nats,
        signer,
        &route,
        serde_json::to_vec(&request)?,
        &scope(&lease.dispatch, ScopeVariant::Exact)?,
        WAIT,
    )
    .await??;
    let response: ProviderCleanupResponse = serde_json::from_slice(&response.payload)?;
    ensure!(
        matches!(response, ProviderCleanupResponse::Cleaned { identity, intent }
        if identity == request.identity && intent == request.intent),
        "actual targeted cleanup did not settle its exact owned identity"
    );
    native.wait_absent(lease.grant.sandbox_id.as_str()).await
}

fn targeted_heartbeat(heartbeat: &WorkerHeartbeat) -> bool {
    heartbeat.provider == ProviderKind::LocalDocker
        && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1
        && heartbeat.supports_lease_epoch.is_supported()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires actual Docker, cached Node22, nats-server and a static SANDBOX_DOCKER_COMMAND_SUPERVISOR; run with --ignored"]
async fn actual_assigned_runtime_readiness_binds_native_scope_and_preserves_elevated_ordinary_work()
-> anyhow::Result<()> {
    let supervisor = Supervisor::configured()?;
    let directory = tempfile::tempdir()?;
    let owner = format!("command-readiness-{}", fresh_id());
    let nats = Nats::start().await?;
    let signer = targeted_process_support::signer()?;
    let mut heartbeats = nats
        .client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::LocalDocker))
        .await?;
    nats.client.flush().await?;
    let mut worker = targeted_process_support::worker_with_supervisor(
        &nats,
        &directory.path().join("worker"),
        &owner,
        &signer,
        Some(supervisor.path()),
    )?;
    let mut native = Native::new(owner.clone()).await?;
    let docker = Docker::connect_with_defaults()?.with_timeout(Duration::from_secs(10));
    let image = docker
        .inspect_image(IMAGE)
        .await?
        .id
        .context("cached Node image has no immutable identity")?;
    ensure!(
        image
            .strip_prefix("sha256:")
            .is_some_and(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())),
        "cached native control image has no canonical digest"
    );
    let mut leases: Vec<Lease> = Vec::new();
    let mut volumes = BTreeSet::new();
    let mut idle_worker = None;
    let mut idle_native = None;
    let mut report = json!({"phase":"actual_worker", "component_only":true,
        "secure_sdk_acceptance":false, "authority_mode":"off", "flags_overridden":false,
        "rpc_negative_checks_earned":false, "fallback_used":false});
    let operation = std::panic::AssertUnwindSafe(async {
        let message = tokio::time::timeout(WAIT, heartbeats.next()).await?.context("actual worker heartbeat absent")?;
        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
        ensure!(worker.is_alive()? && targeted_heartbeat(&heartbeat),
            "actual worker did not earn signed required-epoch targeted intake");
        report["actual_flags"] = json!({"command":heartbeat.profile.isolation.supports_command_cancellation.is_supported(),
            "launch":heartbeat.profile.isolation.supports_command_launch_cancellation.is_supported()});
        let worker_id = heartbeat.worker_id.clone();
        let idle_owner = format!("readiness-idle-{}", fresh_id());
        idle_native = Some(Native::new(idle_owner.clone()).await?);
        let idle = idle_worker.insert(targeted_process_support::worker(&nats,
            &directory.path().join("idle-worker"), &idle_owner, &signer)?);
        let idle_id = tokio::time::timeout(WAIT, async {
            loop {
                let message = heartbeats.next().await.context("idle worker heartbeat stream ended")?;
                let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
                if heartbeat.worker_id != worker_id {
                    ensure!(idle.is_alive()? && targeted_heartbeat(&heartbeat),
                        "second actual worker did not earn independent targeted intake");
                    return Ok::<_, anyhow::Error>(heartbeat.worker_id);
                }
            }
        }).await.context("second actual worker heartbeat absent")??;
        report["second_actual_worker_ready"] = true.into();
        for (name, elevated) in [("safe", false), ("elevated", true)] {
            let lease = allocate(&nats, &signer, &mut native, dispatch(&worker_id, name, elevated, PrivilegedAccess::Denied)?).await?;
            leases.push(lease);
            let lease = leases.last().context("allocated readiness lease absent")?;
            volumes.insert(command_readiness_support::sealed_runtime(&docker, lease.grant.sandbox_id.as_str(), &owner, &supervisor.sha256).await?);
            let inspected = docker.inspect_container(lease.grant.sandbox_id.as_str(), None).await?;
            ensure!(inspected.image.as_deref() == Some(image.as_str()), "actual allocation differs from its cached control image");
            let host = inspected.host_config.context("native HostConfig absent")?;
            ensure!(host.privileged != Some(true) && host.cap_drop.as_ref().is_some_and(|caps|
                caps.iter().any(|cap| command_readiness_support::normalize(cap) == "ALL")), "actual lease lost secure drop-all");
            ensure!(host.cap_add.as_ref().is_some_and(|caps| caps.iter().any(|cap|
                command_readiness_support::normalize(cap) == "SYS_PTRACE")) == elevated,
                "effective native SYS_PTRACE setup differs from its intended control");
            let output = completed(&nats, &signer, lease, &command(lease, ROOT_CONTROL, false, "")?).await?;
            let observed: Value = serde_json::from_slice(&output.stdout)?;
            ensure!(observed["uid"] == 0 && observed["ptrace"] == elevated, "actual uid0/effective capability control failed");
            if elevated {
                let forbidden = format!("require('node:fs').writeFileSync({},'forbidden')", serde_json::to_string(EFFECT)?);
                let response: RunCommandResponse = rpc(&nats, &signer, lease, "run_command",
                    &command(lease, &forbidden, true, "elevated-strong-refusal")?).await?;
                ensure!(matches!(response, RunCommandResponse::Denied(_)), "elevated ordinary lease accepted strong custody");
            } else {
                let output = completed(&nats, &signer, lease, &command(lease, ROOT_CONTROL, true, "sealed-native-control")?).await?;
                let observed: Value = serde_json::from_slice(&output.stdout)?;
                ensure!(observed["uid"] == 0 && observed["ptrace"] == false, "actual sealed native strong control failed");
            }
            no_effect(&nats, &signer, lease).await?;
        }
        report["runtime_controls"] = json!({"safe_uid0":true,"sealed_native_control":true,
            "mounted_artifact_verified":true,"elevated_effective_ptrace":true,"elevated_ordinary_positive":true,
            "elevated_strong_refused_without_effect":true});
        let safe = leases.first().context("safe readiness lease absent")?;
        let elevated = leases.get(1).context("elevated readiness lease absent")?;
        let before = native.owned().await?;
        let denied = send_dispatch(&nats, &signer,
            &dispatch(&worker_id, "privileged-refusal", false, PrivilegedAccess::Granted)?).await?;
        ensure!(!matches!(denied, ProviderDispatchReply::Granted(_)),
            "privileged lease must remain refused before allocation");
        ensure!(native.owned().await? == before, "privileged preallocation refusal changed owned native allocations");
        report["privileged_preallocation_refused"] = true.into();
        let forbidden = format!("require('node:fs').writeFileSync({},'foreign')", serde_json::to_string(EFFECT)?);
        let foreign_request = command(safe, &forbidden, true, "foreign-scope-refusal")?;
        let route = subject(safe, "run_command");
        for (variant, expected) in [
            (ScopeVariant::ForeignTenant, ExistingScopeRefusalReason::LeaseTenantConflict),
            (ScopeVariant::UnknownAttempt, ExistingScopeRefusalReason::StaleAttempt),
        ] {
            let foreign_scope = scope(&safe.dispatch, variant)?;
            match raw(&nats, &signer, &route, serde_json::to_vec(&foreign_request)?, &foreign_scope, Duration::from_secs(1)).await? {
                Ok(message) => assert_existing_scope_refusal(&message.payload, expected)?,
                Err(error) => ensure!(error.kind() == async_nats::RequestErrorKind::TimedOut,
                    "scope control lost the existing actual command handler"),
            }
            no_effect(&nats, &signer, safe).await?;
        }
        report["existing_signed_foreign_and_unknown_scope_no_effect"] = true.into();
        report["phase"] = "actual_command_readiness_rpc".into();
        let safe_binding = safe.binding()?;
        let safe_ready = readiness(&nats, &signer, safe, &safe_binding).await?;
        report["safe_rpc"] = serde_json::to_value(&safe_ready)?;
        match safe_ready {
            Observation::Reply { response: Response::Ready { binding, runtime } } => {
                ensure!(binding == safe_binding && runtime == Runtime { sha256: supervisor.sha256.clone(),
                    protocol: sandbox_command_supervisor::protocol::VERSION }, "Ready did not bind exact user/native/runtime/assignment");
                let subscriptions = nats.subscriptions(&readiness_subject(safe)?).await?;
                ensure!(subscriptions.len() == 1 && subscriptions.first().is_some_and(|found| found.qgroup.is_none()),
                    "only the exact addressed worker may subscribe to this readiness request");
                let elevated_binding = elevated.binding()?;
                ensure!(matches!(readiness(&nats, &signer, elevated, &elevated_binding).await?,
                    Observation::Reply { response: Response::Unsupported { binding, reason: UnsupportedReason::IncompatibleIsolation }}
                    if binding == elevated_binding), "elevated actual lease must be Unsupported, not Ready or probe/storage failure");
                for field in [ForeignField::Tenant, ForeignField::Provider, ForeignField::Worker, ForeignField::Attempt,
                    ForeignField::Epoch, ForeignField::Native] {
                    let foreign = foreign_binding(&safe_binding, field)?;
                    ensure!(matches!(readiness(&nats, &signer, safe, &foreign).await?,
                        Observation::Reply { response: Response::Denied { .. }}), "foreign readiness {field:?} binding must be Denied");
                    no_effect(&nats, &signer, safe).await?;
                }
                let mut unknown = safe.clone();
                unknown.dispatch.lease_id = LeaseId::new("unknown-readiness-lease");
                let unknown_binding = unknown.binding()?;
                ensure!(matches!(readiness(&nats, &signer, &unknown, &unknown_binding).await?,
                    Observation::Reply { response: Response::Denied { .. }}), "unknown lease binding must be Denied");
                no_effect(&nats, &signer, safe).await?;
                let mut misaddressed = safe.clone();
                misaddressed.dispatch.target.as_mut().context("safe readiness lease has no target")?.worker_id = idle_id;
                ensure!(matches!(readiness(&nats, &signer, &misaddressed, &safe_binding).await?,
                    Observation::Reply { response: Response::Denied { .. }}),
                    "the unrelated addressed worker must not borrow another worker's known lease");
                ensure!(idle_native.as_ref().context("idle native fixture absent")?.owned().await?.is_empty(),
                    "misaddressed readiness created work on an unrelated worker");
                let route = readiness_subject(safe)?;
                let original = serde_json::to_vec(&safe_binding)?;
                let mut tampered = safe_binding.clone();
                tampered.run_as_user = Some(SandboxUserName::new("nobody")?);
                let mut headers = async_nats::HeaderMap::new();
                headers.insert(AUTH_HEADER, signer.sign_verb(&route, &original, &scope(&safe.dispatch, ScopeVariant::Exact)?)?);
                match nats.client.send_request(route, async_nats::Request::new().headers(headers)
                    .payload(serde_json::to_vec(&tampered)?.into()).timeout(Some(Duration::from_secs(1)))).await {
                    Ok(message) => ensure!(matches!(serde_json::from_slice::<Response>(&message.payload)?, Response::Denied { .. }),
                        "readiness accepted a user binding changed after signing"),
                    Err(error) => ensure!(error.kind() == async_nats::RequestErrorKind::TimedOut,
                        "user-binding negative lost the proven readiness route"),
                }
                no_effect(&nats, &signer, safe).await?;
                report["rpc_negative_checks_earned"] = true.into();
                Ok(())
            }
            Observation::NoResponders => {
                ensure!(worker.is_alive()? && idle.is_alive()?
                    && nats.subscriptions(&readiness_subject(safe)?).await?.is_empty(),
                    "readiness absence was not an actual live worker with no RPC route");
                no_effect(&nats, &signer, safe).await?;
                report["route_absent_after_positive_runtime_controls"] = true.into();
                anyhow::bail!("actual sealed assigned worker has no public command-readiness RPC after its native controls")
            }
            other => anyhow::bail!("actual safe runtime did not earn typed Ready: {other:?}"),
        }
    }).catch_unwind().await;
    let operation = operation.unwrap_or_else(|_| Err(anyhow::anyhow!("readiness contract panicked")));
    let mut cleanup_errors = Vec::new();
    for lease in leases.iter().rev() {
        if cleanup(&nats, &signer, lease, &native).await.is_err() {
            cleanup_errors.push("targeted_cleanup_failed");
        }
    }
    if let Some(worker) = idle_worker.as_mut()
        && worker.stop_gracefully().await.is_err()
    {
        cleanup_errors.push("idle_worker_shutdown_failed");
    }
    if let Some(native) = idle_native.as_mut()
        && !matches!(native.finish().await, Ok(false))
    {
        cleanup_errors.push("unrelated_worker_native_cleanup_unverified");
    }
    if worker.stop_gracefully().await.is_err() {
        cleanup_errors.push("worker_shutdown_failed");
    }
    let fallback = native.finish().await.unwrap_or_else(|_| {
        cleanup_errors.push("exact_native_cleanup_unverified");
        true
    });
    for volume in &volumes {
        if !matches!(
            docker.inspect_volume(volume).await,
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
        ) {
            cleanup_errors.push("runtime_volume_absence_unverified");
        }
    }
    report["fallback_used"] = fallback.into();
    report["exact_native_cleanup"] = (cleanup_errors.is_empty() && !fallback).into();
    report["cleanup_errors"] = json!(cleanup_errors);
    report["primary_error"] = operation.as_ref().err().map(ToString::to_string).into();
    eprintln!("actual Docker command-readiness receipt: {report}");
    ensure!(
        cleanup_errors.is_empty() && !fallback,
        "readiness contract needs exact product cleanup without fallback"
    );
    operation
}
