//! The native command supervisor on a real Docker lease: strong launches are refused on every isolation that can reach
//! into the helper, the helper's environment and control stay out of guest reach, and a warm reset keeps the framework
//! keepalive while it clears tenant state.

use std::path::Path;

use anyhow::ensure;
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, ExtraCapability, LaunchKey, ProviderDispatchPayload,
    RunCommandRequest, RunCommandResponse,
};
use trogon_sandbox_protocol::subjects::evidence_subject;
use trogon_sandbox_protocol::{DispatchSigner, DispatchVerifier, EnforcementMode, VerbScope};
use trogon_sandbox_providers::{DOCKER_LABEL_LEASE_ID, DOCKER_LABEL_OWNER, DOCKER_LABEL_ROLE};
use trogon_std::env::{ReadEnv, SystemEnv};

use super::*;
use crate::fresh_id::fresh_id;
use crate::support_tests::{lease_request_with, spec_parts};

/// The image every native supervisor lane leases.
const NODE_IMAGE: &str = "docker.io/library/node:22-bookworm-slim";

#[derive(Clone, Copy)]
enum Isolation {
    Default,
    Ptrace,
    All,
    Admin,
    Privileged,
}

struct Fixture {
    _server: trogon_nats::test_support::JetStreamTestServer,
    provider: DockerWorkerProvider,
    lease: LeaseId,
    grant: ProviderLease,
    signer: DispatchSigner,
    verifier: DispatchVerifier,
    scope: VerbScope,
    root: SandboxUserName,
}

/// A bare node lease in `/tmp` with no network, shaped by `isolation`.
fn lease_request(lease: &LeaseId, isolation: Isolation) -> anyhow::Result<LeaseRequest> {
    let mut parts = spec_parts().map_err(|error| anyhow::anyhow!("{error}"))?;
    parts.image = SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified(NODE_IMAGE),
        pull_secret: None,
    };
    parts.boot_mode = trogon_sandbox_contract::BootMode::Bare;
    parts.working_dir = Some("/tmp".into());
    parts.network_policy = Some(NetworkPolicy::DenyAll);
    if matches!(isolation, Isolation::Ptrace | Isolation::All | Isolation::Admin) {
        parts.user = Some("root".into());
    }
    if matches!(isolation, Isolation::Ptrace) {
        parts
            .platform
            .container
            .extra_capabilities
            .push(ExtraCapability::new("SYS_PTRACE")?);
    }
    let spec = trogon_sandbox_contract::SandboxSpec::new(parts).context("native boundary lease spec")?;
    lease_request_with(lease.as_str(), spec).map_err(|error| anyhow::anyhow!("{error}"))
}

/// A privileged container the product never creates, registered as the lease so the refusal path sees it.
async fn privileged_grant(
    provider: &DockerWorkerProvider,
    docker: &Docker,
    worker: &str,
    lease: &LeaseId,
    request: &LeaseRequest,
) -> anyhow::Result<ProviderLease> {
    let mount = provider
        .prepare_runtime_mount(NODE_IMAGE)
        .await?
        .context("runtime mount missing")?;
    let created = docker
        .create_container(
            Some(
                bollard::query_parameters::CreateContainerOptionsBuilder::new()
                    .name(&format!("supervisor-boundary-{}", fresh_id()))
                    .build(),
            ),
            bollard::models::ContainerCreateBody {
                image: Some(NODE_IMAGE.into()),
                entrypoint: Some(vec![]),
                cmd: Some(vec!["sleep".into(), "infinity".into()]),
                labels: Some(HashMap::from([
                    (DOCKER_LABEL_OWNER.into(), worker.to_owned()),
                    (DOCKER_LABEL_LEASE_ID.into(), lease.as_str().to_owned()),
                    (DOCKER_LABEL_ROLE.into(), "workload".into()),
                ])),
                host_config: Some(bollard::models::HostConfig {
                    init: Some(true),
                    auto_remove: Some(true),
                    privileged: Some(true),
                    network_mode: Some("none".into()),
                    binds: Some(vec![mount.binding()]),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await?;
    ensure!(
        created.id.len() == 64 && created.id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "native fixture allocation is not canonical"
    );
    eprintln!("native boundary allocation: owned_container_id={}", created.id);
    docker
        .start_container(&created.id, None::<bollard::query_parameters::StartContainerOptions>)
        .await?;
    provider
        .command_runtime
        .as_ref()
        .context("runtime missing")?
        .verify_mount(docker, &created.id, &mount)
        .await?;
    provider.active.lock().await.insert(
        lease.as_str().to_owned(),
        ActiveDockerLease {
            runtime_mount: Some(mount),
            registration_id: fresh_id(),
            dispatch_target: None,
            dispatch_target_bound: false,
            container_id: created.id.clone(),
            snapshot_digest: None,
            snapshot_reference: None,
            memory_mib: request.spec.memory_mib().get(),
            cpu: 1.0,
            policy_file_path: None,
            sidecar_id: None,
            exposed_port_endpoints: HashMap::new(),
            warm_pool_eligible: false,
            request: Box::new(request.clone()),
        },
    );
    Ok(ProviderLease {
        epoch: None,
        sandbox_id: SandboxId::new(created.id)?,
        evidence_subject: evidence_subject(lease),
        worker_id: None,
        attempt_id: None,
    })
}

impl Fixture {
    async fn new(isolation: Isolation) -> anyhow::Result<Self> {
        let artifact = ReadEnv::var_os(&SystemEnv, "SANDBOX_DOCKER_COMMAND_SUPERVISOR")
            .context("native boundary fixture requires SANDBOX_DOCKER_COMMAND_SUPERVISOR")?;
        let root = SandboxUserName::new("root")?;
        let docker = Docker::connect_with_defaults()?;
        docker.info().await?;
        let server = trogon_nats::test_support::JetStreamTestServer::start().await;
        let client = server.client().await;
        let worker = format!("supervisor-boundary-{}", fresh_id());
        let mut inner = LocalDockerProvider::with_default_engine(NODE_IMAGE.into())?
            .with_owner(worker.clone())
            .with_hardening(ContainerHardening::secure());
        if matches!(isolation, Isolation::All | Isolation::Admin) {
            let mut hardening = inner.hardening().clone();
            hardening.retained_capabilities.push(
                if matches!(isolation, Isolation::All) {
                    "ALL"
                } else {
                    "SYS_ADMIN"
                }
                .into(),
            );
            inner = inner.with_hardening(hardening);
        }
        let runtime = command_runtime::CommandRuntime::load(Path::new(&artifact), worker.clone())?;
        eprintln!(
            "native boundary ownership: planned_runtime_volume={} artifact_digest={}",
            runtime.volume_name(),
            runtime.digest()
        );
        let provider = DockerWorkerProvider::new(
            inner,
            NODE_IMAGE.into(),
            jetstream::new(client.clone()),
            format!("nats://{}", server.address()),
            worker.clone(),
            crate::adapter::build_oci_client(vec![]),
            "docker.io".into(),
            None,
            Arc::new(DigestCache::new(8)),
            None,
            None,
            WarmPoolConfig {
                enabled: false,
                ..Default::default()
            },
        )?
        .with_command_runtime(runtime);
        let lease = LeaseId::new(fresh_id());
        let request = lease_request(&lease, isolation)?;
        let mut seed = [0; 32];
        seed[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        seed[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        let signer = DispatchSigner::new("supervisor-boundary", seed);
        let verifier =
            DispatchVerifier::new(EnforcementMode::Require).with_key(signer.key_id(), &signer.public_key_b64())?;
        let allocation: anyhow::Result<ProviderLease> = async {
            let grant = if matches!(isolation, Isolation::Privileged) {
                privileged_grant(&provider, &docker, &worker, &lease, &request).await?
            } else {
                provider.lease(&request, &lease).await?
            };
            eprintln!("native boundary allocation: owned_container_id={}", grant.sandbox_id);
            provider
                .bind_command_assignment(&ProviderDispatchPayload {
                    lease_id: lease.clone(),
                    request: request.clone(),
                    correlation_id: None,
                    resolved_drives: vec![],
                    target: None,
                })
                .await?;
            Ok(grant)
        }
        .await;
        let grant = match allocation {
            Ok(grant) => grant,
            Err(error) => {
                provider.inner.sweep_owned().await?;
                provider.shutdown_command_runtime().await?;
                return Err(error);
            }
        };
        Ok(Self {
            _server: server,
            provider,
            scope: VerbScope {
                tenant: request.tenant.clone(),
                lease_id: lease.clone(),
                worker_id: None,
                attempt_id: None,
                epoch: None,
            },
            lease,
            grant,
            signer,
            verifier,
            root,
        })
    }

    async fn probe(&self, script: &str, arguments: Vec<String>) -> anyhow::Result<CommandOutput> {
        let mut command = vec!["node".into(), "-e".into(), script.into()];
        command.extend(arguments);
        tokio::time::timeout(
            Duration::from_secs(10),
            self.provider.run_command_for_lease(
                &self.lease,
                command,
                Some("/tmp".into()),
                &CommandEnv::default(),
                5,
                Some(&self.root),
                StreamedOutput::Disabled,
                65536,
            ),
        )
        .await?
    }

    async fn run(&self, request: &RunCommandRequest) -> anyhow::Result<RunCommandResponse> {
        let subject = format!("sandbox.provider.local_docker.run_command.{}", self.lease.as_str());
        let payload = serde_json::to_vec(request)?;
        let header = self.signer.sign_verb(&subject, &payload, &self.scope)?;
        let scope =
            self.verifier
                .verify_verb_scope(&subject, &header, &payload, false, std::time::SystemTime::now())?;
        Ok(self
            .provider
            .run_controlled_command(request, &scope, &subject, &payload)
            .await)
    }

    fn request(&self, key: &str, command: Vec<String>) -> anyhow::Result<RunCommandRequest> {
        Ok(serde_json::from_value(serde_json::json!({
            "lease_id": self.lease.as_str(),
            "command": command,
            "detached": true,
            "require_launch_cancellation": true,
            "launch_key": key,
            "deadline": "lease",
            "run_as_user": self.root,
            "cwd": "/tmp",
        }))?)
    }

    async fn cleanup(&self) -> anyhow::Result<()> {
        let container = self.grant.sandbox_id.as_str();
        if let Some(runtime) = &self.provider.command_runtime {
            eprintln!(
                "native boundary cleanup ownership: owned_container_id={container} owned_runtime_volume={}",
                runtime.volume_name()
            );
        }
        let result = tokio::time::timeout(Duration::from_secs(25), self.provider.cancel(&self.lease)).await;
        let mut reconciled = false;
        match self.provider.docker.inspect_container(container, None).await {
            Ok(inspected) => {
                ensure!(
                    inspected.id.as_deref() == Some(container)
                        && inspected
                            .config
                            .as_ref()
                            .and_then(|config| config.labels.as_ref())
                            .and_then(|labels| labels.get(DOCKER_LABEL_OWNER))
                            == Some(&self.provider.worker_id),
                    "test teardown refused a changed native owner"
                );
                match self
                    .provider
                    .docker
                    .remove_container(
                        container,
                        Some(
                            bollard::query_parameters::RemoveContainerOptionsBuilder::new()
                                .force(true)
                                .v(true)
                                .build(),
                        ),
                    )
                    .await
                {
                    Ok(()) | Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {}
                    Err(error) => return Err(error.into()),
                }
                reconciled = true;
            }
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {}
            Err(error) => return Err(error.into()),
        }
        let absent = matches!(
            self.provider.docker.inspect_container(container, None).await,
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
        );
        let runtime = self.provider.shutdown_command_runtime().await;
        eprintln!(
            "native boundary cleanup: owned_container_id={container} provider_cleanup_confirmed={} exact_container_absent={absent} runtime_removed={} fixture_reconciliation_used={reconciled}",
            matches!(result, Ok(Ok(()))),
            runtime.is_ok(),
        );
        ensure!(absent, "native boundary exact physical container absence unconfirmed");
        runtime?;
        result??;
        ensure!(
            !reconciled,
            "test reconciliation cannot replace successful product cleanup"
        );
        Ok(())
    }

    async fn finish(&self, observed: anyhow::Result<()>) -> anyhow::Result<()> {
        let cleanup = self.cleanup().await;
        match (observed, cleanup) {
            (Err(primary), Err(cleanup)) => {
                eprintln!("native primary assertion: {primary}; separate product cleanup failure: {cleanup}");
                Err(primary.context("native observation failed and product cleanup also failed separately"))
            }
            (Err(primary), Ok(())) => Err(primary),
            (Ok(()), Err(cleanup)) => Err(cleanup),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

/// Reports the uid and the effective, permitted and bounding capabilities of the probe and of PID 1.
const CAPABILITIES: &str = r#"const fs=require('node:fs');const fields=path=>{const status=fs.readFileSync(path,'utf8'),result={};for(const name of ['Uid','CapEff','CapPrm','CapBnd']){const match=status.match(new RegExp('^'+name+':\\s*([^\\n]+)','m'));if(!match)throw Error('capability diagnostic field missing');result[name]=match[1].trim();}return result;};const probe=fields('/proc/self/status');process.stdout.write(JSON.stringify({uid:process.getuid(),effective:probe.CapEff,probe,pid1:fields('/proc/1/status')}));"#;

async fn refuse_effective_isolation(mode: Isolation) -> anyhow::Result<()> {
    let fixture = Fixture::new(mode).await?;
    let observed = async {
        let inspected = fixture
            .provider
            .docker
            .inspect_container(fixture.grant.sandbox_id.as_str(), None)
            .await?;
        ensure!(
            inspected.id.as_deref() == Some(fixture.grant.sandbox_id.as_str()),
            "unsafe-capability control container changed"
        );
        let caps = fixture.probe(CAPABILITIES, vec![]).await?;
        ensure!(caps.exit_code == 0, "effective capability control failed");
        let capabilities: serde_json::Value = serde_json::from_slice(&caps.stdout)?;
        eprintln!(
            "native capability diagnostic: {}",
            serde_json::json!({
                "container_user": inspected.config.as_ref().and_then(|config| config.user.as_deref()),
                "privileged": inspected.host_config.as_ref().and_then(|host| host.privileged),
                "cap_add": inspected.host_config.as_ref().and_then(|host| host.cap_add.as_ref()),
                "cap_drop": inspected.host_config.as_ref().and_then(|host| host.cap_drop.as_ref()),
                "probe_uid": capabilities["uid"],
                "probe": capabilities["probe"],
                "pid1": capabilities["pid1"],
            })
        );
        ensure!(
            capabilities["uid"] == 0,
            "effective capability observer did not run as explicit root"
        );
        let caps = u64::from_str_radix(
            capabilities["effective"]
                .as_str()
                .context("effective capability bytes missing")?,
            16,
        )?;
        let ptrace = caps & (1 << 19) != 0;
        let admin = caps & (1 << 21) != 0;
        ensure!(
            match mode {
                Isolation::Ptrace => ptrace,
                Isolation::All => ptrace && admin,
                Isolation::Admin => admin,
                Isolation::Privileged =>
                    ptrace && admin && inspected.host_config.as_ref().and_then(|host| host.privileged) == Some(true),
                Isolation::Default => false,
            },
            "fixture did not establish actual incompatible effective isolation"
        );
        let ordinary = fixture
            .probe("process.stdout.write('ordinary-control')", vec![])
            .await?;
        ensure!(
            ordinary.exit_code == 0 && ordinary.stdout == b"ordinary-control",
            "ordinary allowed-capability execution changed"
        );
        let path = format!("/tmp/unsafe-strong-{}", fresh_id());
        let request = fixture.request(
            "unsafe-strong",
            vec![
                "node".into(),
                "-e".into(),
                "require('node:fs').writeFileSync(process.argv[1],'released')".into(),
                path.clone(),
            ],
        )?;
        let response = fixture.run(&request).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let released = loop {
            let output = fixture
                .probe(
                    "process.stdout.write(String(require('node:fs').existsSync(process.argv[1])))",
                    vec![path.clone()],
                )
                .await?;
            ensure!(output.exit_code == 0, "native user-effect observer failed");
            if output.stdout == b"true" {
                break true;
            }
            ensure!(output.stdout == b"false", "native user-effect observer output changed");
            if matches!(response, RunCommandResponse::Denied(_)) || tokio::time::Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        ensure!(
            matches!(response, RunCommandResponse::Denied(_)) && !released,
            "actual incompatible isolation admitted strong work or released a user effect"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    fixture.finish(observed).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker, cached node:22-bookworm-slim and NATS; run with --ignored"]
async fn effective_ptrace_refuses_strong_work_and_preserves_ordinary_execution() -> anyhow::Result<()> {
    refuse_effective_isolation(Isolation::Ptrace).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker, cached node:22-bookworm-slim and NATS; run with --ignored"]
async fn effective_all_refuses_strong_work_and_preserves_ordinary_execution() -> anyhow::Result<()> {
    refuse_effective_isolation(Isolation::All).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker, cached node:22-bookworm-slim and NATS; run with --ignored"]
async fn effective_admin_refuses_strong_work_and_preserves_ordinary_execution() -> anyhow::Result<()> {
    refuse_effective_isolation(Isolation::Admin).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker, cached node:22-bookworm-slim and NATS; run with --ignored"]
async fn effective_privileged_refuses_strong_work_and_preserves_ordinary_execution() -> anyhow::Result<()> {
    refuse_effective_isolation(Isolation::Privileged).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker, cached node:22-bookworm-slim and NATS; run with --ignored"]
async fn default_native_supervisor_environment_and_control_remain_inaccessible() -> anyhow::Result<()> {
    let fixture = Fixture::new(Isolation::Default).await?;
    let observed = async {
        let path = format!("/tmp/protected-helper-{}", fresh_id());
        let request = fixture.request("protected-helper", vec!["node".into(), "-e".into(),
            "require('node:fs').writeFileSync(process.argv[1],'started');setInterval(()=>{},1000)".into(), path.clone()])?;
        ensure!(matches!(fixture.run(&request).await?, RunCommandResponse::Started(_)), "default native strong positive did not start");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let output = fixture.probe("process.stdout.write(String(require('node:fs').existsSync(process.argv[1])))", vec![path.clone()]).await?;
            ensure!(output.exit_code == 0, "default user-work observer failed");
            if output.stdout == b"true" { break; }
            ensure!(output.stdout == b"false" && tokio::time::Instant::now() < deadline,
                "default admitted native user work never executed");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let active = fixture.provider.active.lock().await.get(fixture.lease.as_str()).cloned().context("default registration missing")?;
        let owner = command_custody::Owner { tenant: active.request.tenant.clone(), lease_id: fixture.lease.clone(),
            container_id: active.container_id, worker_id: fixture.provider.worker_id.clone(), registration_id: active.registration_id, target: active.dispatch_target };
        let stored = fixture.provider.command_custody.load(&owner, &LaunchKey::new("protected-helper")?).await?.context("default native custody missing")?;
        let native = stored.record.native.context("default native binding missing")?;
        let output = fixture.probe("const fs=require('node:fs'),pid=process.argv[1];const probe=path=>{try{const fd=fs.openSync(path,'r');fs.closeSync(fd);return 'accessible';}catch(error){return error.code;}};process.stdout.write(JSON.stringify({environment:probe('/proc/'+pid+'/environ'),control:probe('/proc/'+pid+'/fd/0')}));", vec![native.helper_pid.to_string()]).await?;
        ensure!(output.exit_code == 0, "default helper isolation observer failed");
        let result: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        ensure!(result["environment"] == "EACCES" && result["control"] == "EACCES", "default helper environment/control was guest accessible");
        let request = CancelCommandLaunchRequest { lease_id: fixture.lease.clone(), launch_key: LaunchKey::new("protected-helper")? };
        let subject = format!("sandbox.provider.local_docker.cancel_command_launch.{}", fixture.lease.as_str());
        let payload = serde_json::to_vec(&request)?;
        let header = fixture.signer.sign_verb(&subject, &payload, &fixture.scope)?;
        let scope = fixture.verifier.verify_verb_scope(&subject, &header, &payload, false, std::time::SystemTime::now())?;
        ensure!(matches!(fixture.provider.cancel_controlled_command(&request, &scope, &subject, &payload).await,
            CancelCommandLaunchResponse::Canceled { .. }), "default protected helper native cleanup did not settle");
        Ok::<_, anyhow::Error>(())
    }.await;
    fixture.finish(observed).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker, cached node:22-bookworm-slim and NATS; run with --ignored"]
async fn configured_bare_warm_reset_preserves_default_init_and_exact_keepalive() -> anyhow::Result<()> {
    let fixture = Fixture::new(Isolation::Default).await?;
    let observed = async {
        let inspected = fixture.provider.docker.inspect_container(fixture.grant.sandbox_id.as_str(), None).await?;
        let actual_init = inspected.host_config.as_ref().and_then(|host| host.init) == Some(true);
        ensure!(inspected.config.as_ref().and_then(|config| config.cmd.as_ref()).is_some_and(|cmd| cmd.iter().map(String::as_str).eq(["sleep", "infinity"])), "Bare fixture does not hold the framework keepalive command");
        let keepalive = fixture.probe("const fs=require('node:fs'),init=process.argv[1]==='true';const stat=pid=>{try{const value=fs.readFileSync('/proc/'+pid+'/stat','utf8'),fields=value.slice(value.lastIndexOf(')')+1).trim().split(/\\s+/);return {pid:Number(pid),start:fields[19],parent:Number(fields[1])};}catch(error){if(error.code==='ENOENT'||error.code==='ESRCH')return;throw error;}};const values=init?fs.readdirSync('/proc').filter(pid=>/^\\d+$/.test(pid)).map(stat).filter(value=>value&&value.parent===1):[stat(1)];if(values.length!==1||!values[0])throw Error('keepalive ancestry is ambiguous');const value=values[0],cmd=fs.readFileSync('/proc/'+value.pid+'/cmdline').toString().split('\\0').filter(Boolean);if(JSON.stringify(cmd)!==JSON.stringify(['sleep','infinity']))throw Error('keepalive command changed');process.stdout.write(JSON.stringify({pid:value.pid,start:value.start}));", vec![actual_init.to_string()]).await?;
        ensure!(keepalive.exit_code == 0, "independent framework keepalive pin failed");
        let keepalive: serde_json::Value = serde_json::from_slice(&keepalive.stdout)?;
        let temporary = format!("/tmp/warm-reset-{}", fresh_id());
        let workspace = format!("{WORKSPACE_MOUNT_PATH}/warm-reset-marker");
        fixture.provider.run_command_for_lease_detached(&fixture.lease, vec!["node".into(), "-e".into(),
            "const fs=require('node:fs');fs.mkdirSync(require('node:path').dirname(process.argv[2]),{recursive:true});fs.writeFileSync(process.argv[2],'tenant');const value=fs.readFileSync('/proc/self/stat','utf8'),start=value.slice(value.lastIndexOf(')')+1).trim().split(/\\s+/)[19];fs.writeFileSync(process.argv[1],JSON.stringify({pid:process.pid,start}));setInterval(()=>{},1000);".into(), temporary.clone(), workspace.clone()], Some("/tmp".into()),
            &CommandEnv::default(), 30, None, StreamedOutput::Disabled, StdinMode::Closed, OutputRetention::Buffer, CommandDeadline::Lease, 65536, None).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let guest = loop {
            let output = fixture.probe("const fs=require('node:fs');process.stdout.write(fs.existsSync(process.argv[1])?fs.readFileSync(process.argv[1],'utf8'):'null');", vec![temporary.clone()]).await?;
            ensure!(output.exit_code == 0, "warm guest identity observer failed");
            let guest: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            if !guest.is_null() { break guest; }
            ensure!(tokio::time::Instant::now() < deadline, "warm guest never started");
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        let before = fixture.probe("const fs=require('node:fs'),guest=JSON.parse(process.argv[1]),keepalive=JSON.parse(process.argv[2]);const alive=value=>{const stat=fs.readFileSync('/proc/'+value.pid+'/stat','utf8');return stat.slice(stat.lastIndexOf(')')+1).trim().split(/\\s+/)[19]===value.start;};process.stdout.write(JSON.stringify({guest:alive(guest),keepalive:alive(keepalive),temporary:fs.existsSync(process.argv[3]),workspace:fs.existsSync(process.argv[4])}));", vec![guest.to_string(), keepalive.to_string(), temporary.clone(), workspace.clone()]).await?;
        ensure!(before.exit_code == 0, "warm reset independent pre-state pin failed");
        let before: serde_json::Value = serde_json::from_slice(&before.stdout)?;
        ensure!(before["guest"] == true && before["keepalive"] == true && before["temporary"] == true && before["workspace"] == true,
            "warm reset did not establish exact live guest/framework and file controls");
        let reset = tokio::time::timeout(Duration::from_secs(10), fixture.provider.reset_container_in_place(fixture.grant.sandbox_id.as_str())).await;
        let same_running = match fixture.provider.docker.inspect_container(fixture.grant.sandbox_id.as_str(), None).await {
            Ok(after) => after.id.as_deref() == Some(fixture.grant.sandbox_id.as_str())
                && after.state.as_ref().and_then(|state| state.running) == Some(true),
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => false,
            Err(error) => return Err(error.into()),
        };
        ensure!(same_running, "warm reset killed the exact framework keepalive and canonical container");
        reset??;
        let output = fixture.probe("const fs=require('node:fs'),keepalive=JSON.parse(process.argv[1]),guest=JSON.parse(process.argv[2]);const alive=value=>{try{const stat=fs.readFileSync('/proc/'+value.pid+'/stat','utf8');return stat.slice(stat.lastIndexOf(')')+1).trim().split(/\\s+/)[19]===value.start;}catch(error){if(error.code==='ENOENT'||error.code==='ESRCH')return false;throw error;}};process.stdout.write(JSON.stringify({keepalive:alive(keepalive),guest:alive(guest),temporary:fs.existsSync(process.argv[3]),workspace:fs.existsSync(process.argv[4])}));",
            vec![keepalive.to_string(), guest.to_string(), temporary, workspace]).await?;
        ensure!(output.exit_code == 0, "warm reset post-state observer failed");
        let result: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        ensure!(result["keepalive"] == true && result["guest"] == false && result["temporary"] == false && result["workspace"] == false,
            "warm reset changed framework identity or retained tenant state");
        ensure!(!actual_init, "configuring the native helper changed default Bare init policy");
        Ok::<_, anyhow::Error>(())
    }.await;
    fixture.finish(observed).await
}
