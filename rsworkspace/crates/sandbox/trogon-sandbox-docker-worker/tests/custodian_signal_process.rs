#![cfg(unix)]

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::time::Duration;

use anyhow::{Context, ensure};
use bollard::Docker;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use futures::{FutureExt, StreamExt};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    BootMode, CancelIntent, DispatchAttemptId, DispatchTarget, ExecutionId, KillCommandRequest, KillCommandResponse,
    LeaseAttemptIdentity, LeaseEpoch, LeaseId, LeaseRequest, LeaseRequestKey, NetworkPolicy, PollCommandRequest,
    PollCommandResponse, ProviderCleanupRequest, ProviderCleanupResponse, ProviderDispatchPayload,
    ProviderDispatchReply, ProviderKind, ProviderLease, RunCommandRequest, RunCommandResponse, SandboxSpec,
    SandboxSpecParts, SandboxTags, TenantId, UnixSignal, WorkerDispatchProtocol, WorkerHeartbeat, WorkspaceSource,
};
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner, OP_LEASE_DISPATCH, VerbScope, subjects};

mod command_readiness_support;
mod targeted_process_support;

use command_readiness_support::{Supervisor, sealed_runtime};
use targeted_process_support::{IMAGE, Native, Nats, fresh_id};

const WAIT: Duration = Duration::from_secs(15);
const IDENTITY: &str = r#"
const fs=require('node:fs');
function identity(pid){
  const stat=fs.readFileSync('/proc/'+pid+'/stat','utf8');
  const fields=stat.slice(stat.lastIndexOf(')')+1).trim().split(/\s+/);
  const status=fs.readFileSync('/proc/'+pid+'/status','utf8');
  return {pid:Number(pid),start:fields[19],uid:Number(status.match(/^Uid:\s*(\d+)/m)[1]),
    ppid:Number(fields[1]),group:Number(fields[2]),session:Number(fields[3])};
}
"#;
const SENTINEL: &str = r#"
fs.writeFileSync('/tmp/custodian-sentinel.json',JSON.stringify(identity(process.pid)));
setInterval(()=>{},100);
"#;
const CHILD: &str = r#"
fs.writeFileSync('/tmp/custodian-child.json',JSON.stringify(identity(process.pid)));
setInterval(()=>{
  if(fs.existsSync('/tmp/custodian-effect-go'))fs.writeFileSync('/tmp/custodian-forbidden-effect','child');
},20);
"#;
const ATTACK: &str = r#"
const cp=require('node:child_process');
const status=fs.readFileSync('/proc/self/status','utf8');
const root=identity(process.pid),helper=identity(process.ppid);
const child=cp.spawn(process.execPath,['-e',__CHILD_CODE__],{detached:true,stdio:'ignore'});
child.unref();
fs.writeFileSync('/tmp/custodian-root.json',JSON.stringify({root,helper,
  cap_eff:status.match(/^CapEff:\s*([0-9a-f]+)$/m)[1]}));
let attempted=false;
setInterval(()=>{
  if(!attempted&&fs.existsSync('/tmp/custodian-attack-go')){
    attempted=true;
    let outcome;
    try{process.kill(helper.pid,'SIGKILL');outcome='delivered';}
    catch(error){outcome=error.code==='EPERM'?'permission_denied':'unexpected_error';}
    fs.writeFileSync('/tmp/custodian-signal.json',JSON.stringify({outcome}));
  }
  if(fs.existsSync('/tmp/custodian-effect-go'))fs.writeFileSync('/tmp/custodian-forbidden-effect','root');
},20);
"#;
const OBSERVE: &str = r#"
function read(path){try{return JSON.parse(fs.readFileSync(path,'utf8'));}catch(error){if(error.code==='ENOENT')return null;throw error;}}
function alive(pin){
  if(!pin)return false;
  try{
    const stat=fs.readFileSync('/proc/'+pin.pid+'/stat','utf8');
    const fields=stat.slice(stat.lastIndexOf(')')+1).trim().split(/\s+/);
    return fields[19]===pin.start&&fields[0]!=='Z'&&fields[0]!=='X';
  }catch(error){if(error.code==='ENOENT'||error.code==='ESRCH')return false;throw error;}
}
const root=read('/tmp/custodian-root.json'),child=read('/tmp/custodian-child.json'),sentinel=read('/tmp/custodian-sentinel.json');
process.stdout.write(JSON.stringify({root,child,sentinel,signal:read('/tmp/custodian-signal.json'),
  helper_alive:alive(root&&root.helper),root_alive:alive(root&&root.root),
  child_alive:alive(child),sentinel_alive:alive(sentinel),
  effect:fs.existsSync('/tmp/custodian-forbidden-effect')}));
"#;

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin {
    pid: u32,
    start: String,
    uid: u32,
    ppid: u32,
    group: u32,
    session: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Root {
    root: Pin,
    helper: Pin,
    cap_eff: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum SignalOutcome {
    Delivered,
    PermissionDenied,
    UnexpectedError,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignalAttempt {
    outcome: SignalOutcome,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    root: Option<Root>,
    child: Option<Pin>,
    sentinel: Option<Pin>,
    signal: Option<SignalAttempt>,
    helper_alive: bool,
    root_alive: bool,
    child_alive: bool,
    sentinel_alive: bool,
    effect: bool,
}

struct Lease {
    dispatch: ProviderDispatchPayload,
    grant: ProviderLease,
}

impl Lease {
    fn target(&self) -> anyhow::Result<&DispatchTarget> {
        self.dispatch
            .target
            .as_ref()
            .context("custodian lease has no assigned target")
    }

    fn scope(&self) -> anyhow::Result<VerbScope> {
        let target = self.target()?;
        Ok(VerbScope {
            tenant: self.dispatch.request.tenant.clone(),
            lease_id: self.dispatch.lease_id.clone(),
            worker_id: Some(target.worker_id.clone()),
            attempt_id: Some(target.attempt_id.clone()),
            epoch: target.epoch,
        })
    }

    fn native(&self) -> &str {
        self.grant.sandbox_id.as_str()
    }
}

async fn rpc<T: Serialize, R: DeserializeOwned>(
    nats: &Nats,
    signer: &DispatchSigner,
    lease: &Lease,
    verb: &str,
    request: &T,
) -> anyhow::Result<R> {
    let subject = format!(
        "sandbox.provider.{}.{verb}.{}",
        subjects::provider_token(ProviderKind::LocalDocker),
        lease.dispatch.lease_id.as_str()
    );
    let payload = serde_json::to_vec(request)?;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(AUTH_HEADER, signer.sign_verb(&subject, &payload, &lease.scope()?)?);
    let message = nats
        .client
        .send_request(
            subject,
            async_nats::Request::new()
                .headers(headers)
                .payload(payload.into())
                .timeout(Some(WAIT)),
        )
        .await?;
    ensure!(message.payload.len() <= 65536, "public reply exceeded its bound");
    serde_json::from_slice(&message.payload)
        .map_err(|_| anyhow::anyhow!("public command reply violated its typed contract"))
}

fn command(lease: &Lease, code: String, strong: bool, key: &str) -> anyhow::Result<RunCommandRequest> {
    Ok(serde_json::from_value(json!({
        "lease_id":lease.dispatch.lease_id,"command":["node","-e",code],
        "cwd":"/tmp","run_as_user":"root","detached":true,"stdin":"closed",
        "timeout_seconds":120,"launch_key":if strong {Some(key)} else {None},
        "require_launch_cancellation":strong,
    }))?)
}

async fn start(
    nats: &Nats,
    signer: &DispatchSigner,
    lease: &Lease,
    request: &RunCommandRequest,
) -> anyhow::Result<ExecutionId> {
    let response: RunCommandResponse = rpc(nats, signer, lease, "run_command", request).await?;
    let RunCommandResponse::Started(started) = response else {
        anyhow::bail!("healthy actual strong or sentinel control did not start")
    };
    Ok(started.execution_id)
}

async fn native_json(docker: &Docker, lease: &Lease, source: &str) -> anyhow::Result<Value> {
    let exec = docker
        .create_exec(
            lease.native(),
            CreateExecOptions {
                cmd: Some(vec!["node", "-e", source]),
                user: Some("0"),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await?;
    let StartExecResults::Attached { mut output, .. } = docker.start_exec(&exec.id, None::<StartExecOptions>).await?
    else {
        anyhow::bail!("actual native observer transport was detached")
    };
    let mut bytes = Vec::new();
    while let Some(chunk) = output.next().await {
        match chunk? {
            bollard::container::LogOutput::StdOut { message } => {
                ensure!(
                    bytes.len() + message.len() <= 65536,
                    "native observer output exceeded its bound"
                );
                bytes.extend_from_slice(&message);
            }
            bollard::container::LogOutput::StdErr { message } if message.is_empty() => {}
            _ => anyhow::bail!("native observer failed its controlled execution"),
        }
    }
    ensure!(
        docker.inspect_exec(&exec.id).await?.exit_code == Some(0),
        "native observer did not complete cleanly"
    );
    serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("native observer returned malformed bounded evidence"))
}

async fn snapshot(docker: &Docker, lease: &Lease) -> anyhow::Result<Snapshot> {
    serde_json::from_value(native_json(docker, lease, &format!("{IDENTITY}\n{OBSERVE}")).await?)
        .map_err(|_| anyhow::anyhow!("native snapshot violated its strict evidence schema"))
}

async fn wait_snapshot(
    docker: &Docker,
    lease: &Lease,
    predicate: impl Fn(&Snapshot) -> bool,
) -> anyhow::Result<Snapshot> {
    tokio::time::timeout(WAIT, async {
        loop {
            let found = snapshot(docker, lease).await?;
            if predicate(&found) {
                return Ok(found);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("native evidence deadline expired")?
}

/// The two files the guest program waits on before it acts.
#[derive(Clone, Copy)]
enum Barrier {
    Attack,
    Effect,
}

async fn barrier(docker: &Docker, lease: &Lease, barrier: Barrier) -> anyhow::Result<()> {
    let path = match barrier {
        Barrier::Effect => "/tmp/custodian-effect-go",
        Barrier::Attack => "/tmp/custodian-attack-go",
    };
    let source = format!(
        "require('node:fs').writeFileSync({},'go');process.stdout.write('true');",
        serde_json::to_string(path)?
    );
    ensure!(
        native_json(docker, lease, &source).await? == true,
        "native barrier failed"
    );
    Ok(())
}

fn poll_class(reply: &PollCommandResponse) -> &'static str {
    match reply {
        PollCommandResponse::Running => "running",
        PollCommandResponse::Completed(_) => "completed",
        PollCommandResponse::NotFound => "not_found",
        PollCommandResponse::Denied(_) => "denied",
    }
}

fn kill_class(reply: &KillCommandResponse) -> &'static str {
    match reply {
        KillCommandResponse::Signaled => "signaled",
        KillCommandResponse::AlreadyCompleted => "already_completed",
        KillCommandResponse::NotFound => "not_found",
        KillCommandResponse::Denied(_) => "denied",
    }
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
    let subject = subjects::worker_cleanup(ProviderKind::LocalDocker, &target.worker_id);
    let payload = serde_json::to_vec(&request)?;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(AUTH_HEADER, signer.sign_verb(&subject, &payload, &lease.scope()?)?);
    let message = nats
        .client
        .send_request(
            subject,
            async_nats::Request::new()
                .headers(headers)
                .payload(payload.into())
                .timeout(Some(WAIT)),
        )
        .await?;
    ensure!(
        matches!(serde_json::from_slice::<ProviderCleanupResponse>(&message.payload)?,
        ProviderCleanupResponse::Cleaned {identity,intent} if identity==request.identity&&intent==request.intent),
        "exact targeted cleanup did not settle owned identity"
    );
    native.wait_absent(lease.native()).await
}

fn dispatch(worker_id: &trogon_sandbox_contract::WorkerId) -> anyhow::Result<ProviderDispatchPayload> {
    let lease_id = LeaseId::new("custodian-signal-lease");
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
        privileged: Default::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform: Default::default(),
        idle_timeout_seconds: None,
        no_ttl: Default::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: Default::default(),
    })?;
    Ok(ProviderDispatchPayload {
        lease_id: lease_id.clone(),
        request: LeaseRequest {
            operation_id: None,
            tenant: TenantId::new("custodian-signal-tenant")?,
            provider_pref: vec![ProviderKind::LocalDocker],
            spec,
            ttl_seconds: 120,
            idempotency_key: LeaseRequestKey::new(lease_id.as_str())?,
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
            worker_id: worker_id.clone(),
            attempt_id: DispatchAttemptId::new("custodian-signal-attempt")?,
            epoch: Some(LeaseEpoch::FIRST),
        }),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires actual Docker, cached Node22, nats-server and a static SANDBOX_DOCKER_COMMAND_SUPERVISOR; run with --ignored"]
async fn actual_guest_cannot_destroy_strong_command_custody_and_escape_public_kill() -> anyhow::Result<()> {
    let supervisor = Supervisor::configured()?;
    ensure!(
        supervisor.path().is_absolute(),
        "configured supervisor must be an absolute artifact path"
    );
    let directory = tempfile::tempdir()?;
    let owner = format!("custodian-signal-{}", fresh_id());
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
    let mut lease = None;
    let mut volume = None;
    let mut report = json!({"phase":"actual_worker","component_only":true,"secure_sdk_acceptance":false,
        "authority_mode":"off","flags_overridden":false,"semantic_checks_earned":false,"fallback_used":false});
    let operation = std::panic::AssertUnwindSafe(async {
        let message = tokio::time::timeout(WAIT,heartbeats.next()).await?.context("actual worker heartbeat absent")?;
        let heartbeat:WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
        ensure!(worker.is_alive()?&&heartbeat.provider==ProviderKind::LocalDocker
            &&heartbeat.dispatch_protocol==WorkerDispatchProtocol::TargetedV1&&heartbeat.supports_lease_epoch.is_supported(),
            "actual worker did not earn signed required-epoch targeted intake");
        report["actual_flags"]=json!({"command":heartbeat.profile.isolation.supports_command_cancellation.is_supported(),
            "launch":heartbeat.profile.isolation.supports_command_launch_cancellation.is_supported()});
        let worker_id=heartbeat.worker_id.clone();
        let dispatch=dispatch(&worker_id)?;
        native.register_lease(dispatch.lease_id.as_str());
        let payload=signer.sign_payload(&serde_json::to_vec(&dispatch)?,OP_LEASE_DISPATCH)?;
        let message=nats.client.send_request(subjects::worker_dispatch(ProviderKind::LocalDocker,&worker_id),
            async_nats::Request::new().payload(payload.into()).timeout(Some(WAIT))).await?;
        let reply:ProviderDispatchReply=serde_json::from_slice(&message.payload)?;
        let ProviderDispatchReply::Granted(grant)=reply else {anyhow::bail!("actual signed targeted safe allocation refused");};
        native.register_id(grant.sandbox_id.as_str());
        let lease=&*lease.insert(Lease {dispatch,grant});
        native.running(lease.native(),lease.dispatch.lease_id.as_str()).await?;
        let target=lease.target()?;
        ensure!(lease.grant.worker_id.as_ref()==Some(&target.worker_id)
            &&lease.grant.attempt_id.as_ref()==Some(&target.attempt_id)&&lease.grant.epoch==target.epoch,
            "actual native receipt changed its assigned target");
        volume=Some(sealed_runtime(&docker,lease.native(),&owner,&supervisor.sha256).await?);
        let inspected=docker.inspect_container(lease.native(),None).await?;
        let image=docker.inspect_image(IMAGE).await?.id.context("cached control image has no identity")?;
        ensure!(inspected.image.as_deref()==Some(image.as_str()),"actual native control changed image");
        let host=inspected.host_config.context("actual HostConfig absent")?;
        ensure!(host.privileged!=Some(true)&&host.cap_drop.as_ref().is_some_and(|caps|caps.iter().any(|cap|command_readiness_support::normalize(cap)=="ALL"))
            &&host.cap_add.as_ref().is_some_and(|caps| {
                let mut actual=caps.iter().map(|cap|command_readiness_support::normalize(cap)).collect::<Vec<_>>();
                actual.sort();
                actual==["CHOWN","DAC_OVERRIDE","FOWNER","FSETID","KILL","SETGID","SETUID"]
            }),"actual native control lost its documented secure retained-capability profile");
        let control=start(&nats,&signer,lease,&command(lease,"process.stdout.write('healthy')".into(),true,"positive-strong-control")?).await?;
        tokio::time::timeout(WAIT,async {
            loop {
                let response:PollCommandResponse=rpc(&nats,&signer,lease,"poll_command",&PollCommandRequest {lease_id:lease.dispatch.lease_id.clone(),execution_id:control.clone()}).await?;
                match response {
                    PollCommandResponse::Running=>tokio::time::sleep(Duration::from_millis(20)).await,
                    PollCommandResponse::Completed(output)=>{ensure!(output.exit_code==0&&output.stdout==b"healthy"&&output.stderr.is_empty()&&output.truncated.is_complete()&&output.timed_out.is_within_deadline(),"native positive strong command did not finish cleanly");return Ok::<_,anyhow::Error>(());},
                    _=>anyhow::bail!("native positive control lost execution receipt"),
                }
            }
        }).await.context("positive strong completion deadline")??;
        let sentinel_id=start(&nats,&signer,lease,&command(lease,format!("{IDENTITY}\n{SENTINEL}"),false,"")?).await?;
        let child=format!("{IDENTITY}\n{CHILD}");
        let attack=ATTACK.replace("__CHILD_CODE__",&serde_json::to_string(&child)?);
        let execution_id=start(&nats,&signer,lease,&command(lease,format!("{IDENTITY}\n{attack}"),true,"guest-custodian-signal")?).await?;
        let before=wait_snapshot(&docker,lease,|found|found.helper_alive&&found.root_alive&&found.child_alive&&found.sentinel_alive).await?;
        let root=before.root.as_ref().context("native root pin absent")?;
        let child=before.child.as_ref().context("native child pin absent")?;
        let sentinel=before.sentinel.as_ref().context("native sentinel pin absent")?;
        ensure!(root.root.uid==0&&root.helper.uid==0&&child.uid==0&&sentinel.uid==0
            &&root.root.ppid==root.helper.pid&&child.ppid==root.root.pid
            &&child.group==child.pid&&child.session==child.pid
            &&[root.root.pid,root.helper.pid,child.pid].iter().all(|pid|*pid!=sentinel.pid)
            &&u64::from_str_radix(&root.cap_eff,16)?==0xfb&&!before.effect&&before.signal.is_none(),
            "native same-UID safe isolation, detached owned child or independent sentinel control failed");
        report["positive_controls"]=json!({"sealed_runtime":true,"strong_completion":true,
            "actual_uid0":true,"default_retained_capabilities":true,"effective_mask_matches_default":true,"helper_same_uid":true,
            "direct_parent_pinned":true,"owned_child_detached":true,"independent_sentinel":true});
        report["phase"]="guest_signal_then_public_kill".into();
        barrier(&docker,lease,Barrier::Attack).await?;
        let attacked=wait_snapshot(&docker,lease,|found|found.signal.is_some()).await?;
        let signal=attacked.signal.as_ref().context("guest signal attempt absent")?.outcome;
        ensure!(signal!=SignalOutcome::UnexpectedError,"guest signal failed for an unexpected setup reason");
        let attacked=if signal==SignalOutcome::Delivered {wait_snapshot(&docker,lease,|found|!found.helper_alive).await?} else {attacked};
        ensure!(attacked.root.as_ref()==Some(root)&&attacked.child.as_ref()==Some(child)
            &&attacked.sentinel.as_ref()==Some(sentinel)&&attacked.sentinel_alive,
            "guest signal changed pinned native identities or harmed independent sentinel");
        report["guest_signal"]=serde_json::to_value(signal)?;
        report["helper_alive_after_guest_signal"]=attacked.helper_alive.into();
        report["root_alive_after_guest_signal"]=attacked.root_alive.into();
        report["child_alive_after_guest_signal"]=attacked.child_alive.into();
        let polled=rpc::<_,PollCommandResponse>(&nats,&signer,lease,"poll_command",&PollCommandRequest {lease_id:lease.dispatch.lease_id.clone(),execution_id:execution_id.clone()}).await;
        report["poll_before_public_kill"]=polled.as_ref().map(poll_class).unwrap_or("transport_or_contract_failure").into();
        let killed=rpc::<_,KillCommandResponse>(&nats,&signer,lease,"kill_command",&KillCommandRequest {lease_id:lease.dispatch.lease_id.clone(),execution_id:execution_id.clone(),signal:UnixSignal::Kill}).await;
        report["public_kill"]=killed.as_ref().map(kill_class).unwrap_or("transport_or_contract_failure").into();
        let polled=rpc::<_,PollCommandResponse>(&nats,&signer,lease,"poll_command",&PollCommandRequest {lease_id:lease.dispatch.lease_id.clone(),execution_id}).await;
        report["poll_after_public_kill"]=polled.as_ref().map(poll_class).unwrap_or("transport_or_contract_failure").into();
        barrier(&docker,lease,Barrier::Effect).await?;
        let after=tokio::time::timeout(Duration::from_secs(2),async {
            loop {
                let found=snapshot(&docker,lease).await?;
                if found.effect||(!found.root_alive&&!found.child_alive) {return Ok::<_,anyhow::Error>(found);}
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.context("post-kill native evidence deadline")??;
        ensure!(after.root.as_ref()==Some(root)&&after.child.as_ref()==Some(child)
            &&after.sentinel.as_ref()==Some(sentinel),"post-kill observer changed pinned evidence");
        report["root_alive_after_public_kill"]=after.root_alive.into();
        report["child_alive_after_public_kill"]=after.child_alive.into();
        report["effect_after_public_kill"]=after.effect.into();
        report["sentinel_preserved"]=after.sentinel_alive.into();
        report["semantic_checks_earned"]=true.into();
        let accepted=matches!(killed,Ok(KillCommandResponse::Signaled|KillCommandResponse::AlreadyCompleted))
            &&matches!(polled,Ok(PollCommandResponse::Completed(_)))
            &&!after.root_alive&&!after.child_alive&&!after.effect&&after.sentinel_alive;
        let sentinel_stop:KillCommandResponse=rpc(&nats,&signer,lease,"kill_command",&KillCommandRequest {lease_id:lease.dispatch.lease_id.clone(),execution_id:sentinel_id,signal:UnixSignal::Kill}).await?;
        ensure!(matches!(sentinel_stop,KillCommandResponse::Signaled|KillCommandResponse::AlreadyCompleted),"independent public sentinel cancellation failed");
        wait_snapshot(&docker,lease,|found|!found.sentinel_alive).await?;
        report["independent_sentinel_cleanup"]=true.into();
        ensure!(accepted,"guest destroyed strong command custody and public kill did not retire its exact owned family without effect");
        Ok(())
    }).catch_unwind().await;
    let operation = operation.unwrap_or_else(|_| Err(anyhow::anyhow!("native custodian signal contract panicked")));
    let mut cleanup_errors = Vec::new();
    if let Some(lease) = &lease
        && cleanup(&nats, &signer, lease, &native).await.is_err()
    {
        cleanup_errors.push("exact_targeted_cleanup_failed");
    }
    if worker.stop_gracefully().await.is_err() {
        cleanup_errors.push("worker_shutdown_failed");
    }
    let fallback = native.finish().await.unwrap_or_else(|_| {
        cleanup_errors.push("exact_native_cleanup_unverified");
        true
    });
    if let Some(volume) = &volume
        && !matches!(
            docker.inspect_volume(volume).await,
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
        )
    {
        cleanup_errors.push("runtime_volume_absence_unverified");
    }
    report["fallback_used"] = fallback.into();
    report["exact_native_cleanup"] = (cleanup_errors.is_empty() && !fallback).into();
    report["cleanup_errors"] = json!(cleanup_errors);
    report["primary_error"] = operation.as_ref().err().map(ToString::to_string).into();
    eprintln!("actual Docker custodian signal receipt: {report}");
    ensure!(
        cleanup_errors.is_empty() && !fallback,
        "custodian signal regression requires exact product cleanup without fallback"
    );
    operation
}
