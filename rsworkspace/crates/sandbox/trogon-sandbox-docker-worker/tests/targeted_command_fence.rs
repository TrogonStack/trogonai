#![cfg(unix)]

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, ensure};
use bollard::Docker;
use futures::{FutureExt, StreamExt};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use sha2::{Digest, Sha256};
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CancelIntent, DispatchAttemptId, DispatchTarget,
    KillCommandRequest, KillCommandResponse, LaunchKey, LeaseAttemptIdentity, LeaseEpoch, LeaseId, LeasePersistence,
    OutputTruncation, ProviderCleanupRequest, ProviderCleanupResponse, ProviderDispatchPayload, ProviderDispatchReply,
    ProviderKind, ProviderLease, RestartCommandRequest, RestartCommandResponse, RunCommandRequest, RunCommandResponse,
    RunCommandStartedResponse, UnixSignal, WorkerDispatchProtocol, WorkerHeartbeat, WorkerId,
};
use trogon_sandbox_protocol::subjects;
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner, OP_LEASE_DISPATCH, VerbScope};
use trogon_sandbox_providers::ContainerHardening;

mod targeted_process_support;
mod trusted_observer_support;
use targeted_process_support::{Native as NativeAssets, Nats, fresh_id};
use trusted_observer_support::{Binding, Identity, Native, Observer, Operation, Request, Response, TrackedState};

const FAMILY: &str = r#"const fs=require('node:fs'),cp=require('node:child_process'),base=process.argv[1],marker=process.argv[2];
fs.mkdirSync(require('node:path').dirname(base),{recursive:true});
const child="const fs=require('node:fs'),base=process.argv[1],marker=process.argv[2];process.on('SIGTERM',()=>{});const v=fs.readFileSync('/proc/self/stat','utf8');fs.writeFileSync(base+'.child.json.tmp',JSON.stringify({pid:process.pid,startTime:v.slice(v.lastIndexOf(')')+1).trim().split(/\\s+/)[19],marker}));fs.renameSync(base+'.child.json.tmp',base+'.child.json');setInterval(()=>{},1000);";
process.on('SIGTERM',()=>{});cp.spawn(process.execPath,['-e',child,base,marker],{stdio:'ignore'});
const v=fs.readFileSync('/proc/self/stat','utf8');fs.appendFileSync(base+'.starts',marker+'\n');
fs.writeFileSync(base+'.root.json.tmp',JSON.stringify({pid:process.pid,startTime:v.slice(v.lastIndexOf(')')+1).trim().split(/\s+/)[19],marker}));fs.renameSync(base+'.root.json.tmp',base+'.root.json');setInterval(()=>{},1000);"#;

struct FenceSwap {
    canonical: PathBuf,
    original: PathBuf,
    bytes: Vec<u8>,
}

impl FenceSwap {
    fn replace(canonical: &Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(canonical)?;
        ensure!(!bytes.is_empty(), "positive assignment did not persist its fence");
        let original = canonical.with_extension("retained-original");
        std::fs::rename(canonical, &original)?;
        let swap = Self {
            canonical: canonical.into(),
            original,
            bytes,
        };
        std::fs::write(&swap.canonical, [])?;
        ensure!(swap.present()?, "fence fault did not retain exact original authority");
        Ok(swap)
    }

    fn present(&self) -> anyhow::Result<bool> {
        let original = std::fs::metadata(&self.original)?;
        let canonical = std::fs::metadata(&self.canonical)?;
        Ok(original.ino() != canonical.ino() && std::fs::read(&self.original)? == self.bytes && canonical.len() == 0)
    }

    fn restore(&mut self) -> anyhow::Result<()> {
        if self.original.exists() {
            std::fs::remove_file(&self.canonical)?;
            std::fs::rename(&self.original, &self.canonical)?;
        }
        Ok(())
    }
}

impl Drop for FenceSwap {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

struct Family {
    request: Request,
    observer: Arc<Observer>,
    key: LaunchKey,
    command: RunCommandRequest,
}

impl Family {
    async fn new(docker: &Docker, binding: &Binding, image: &str, key: &str, strong: bool) -> anyhow::Result<Self> {
        let nonce = uuid::Uuid::now_v7().simple().to_string();
        let marker = format!("{}-{nonce}", if strong { "owned" } else { "sentinel" });
        let base = format!("/tmp/named-cancellation-{nonce}/{marker}");
        let observer = Observer::create(
            docker.clone(),
            binding.clone(),
            image.into(),
            format!("fence-observer-{}", fresh_id()),
        )
        .await?;
        let request = Request {
            kind: Operation::NativeFamily,
            request_id: fresh_id(),
            lease_id: binding.lease.clone(),
            container_id: binding.container.clone(),
            marker: marker.clone(),
            paths: [
                format!("{base}.root.json"),
                format!("{base}.child.json"),
                format!("{base}.starts"),
            ],
            tracked: vec![],
        };
        let command = serde_json::from_value(json!({
            "lease_id": binding.lease,
            "command": ["node","-e",FAMILY,base,marker],
            "env": {"SANDBOX_ACCEPTANCE_FAMILY": marker},
            "detached": true,
            "deadline": "lease",
            "process": {"name": key},
            "launch_key": key,
            "require_launch_cancellation": strong,
        }))?;
        Ok(Self {
            request,
            observer,
            key: LaunchKey::new(key)?,
            command,
        })
    }

    async fn observation(&self, tracked: &[Identity]) -> anyhow::Result<(Native, bool)> {
        let mut request = self.request.clone();
        request.request_id = fresh_id();
        request.tracked = tracked.to_vec();
        match self.observer.observe(request).await {
            Response::Observed {
                native,
                proof,
                observer_absent: true,
                ..
            } => Ok((native, proof.root_child_linked)),
            Response::Refused { code, .. } => {
                anyhow::bail!("independent native family observer refused: {code:?}")
            }
            Response::Failed { code, .. } => {
                anyhow::bail!("independent native family observer failed: {code:?}")
            }
            _ => anyhow::bail!("independent native observer did not confirm exact teardown"),
        }
    }

    async fn snapshot(&self, tracked: &[Identity]) -> anyhow::Result<Native> {
        Ok(self.observation(tracked).await?.0)
    }

    async fn ready(&self, starts: usize) -> anyhow::Result<Native> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let (native, root_child_linked) = self.observation(&[]).await?;
                if native.ready
                    && native.records.len() == 2
                    && native.members.len() == 2
                    && native.starts.len() == starts
                    && root_child_linked
                    && Self::pins(&native)
                        .iter()
                        .all(|identity| native.members.contains(identity))
                {
                    return Ok::<_, anyhow::Error>(native);
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .context("actual command family did not establish its positive native control")?
    }

    fn pins(native: &Native) -> Vec<Identity> {
        native
            .records
            .iter()
            .map(|record| Identity {
                pid: record.pid,
                start_time: record.start_time.clone(),
            })
            .collect()
    }

    async fn preserved(&self, before: &Native) -> anyhow::Result<bool> {
        let native = self.snapshot(&Self::pins(before)).await?;
        Ok(native.starts == before.starts
            && native.members == before.members
            && native
                .tracked
                .iter()
                .all(|tracked| tracked.state == TrackedState::Alive))
    }

    async fn absent(&self, before: &Native) -> anyhow::Result<bool> {
        let native = self.snapshot(&Self::pins(before)).await?;
        Ok(native.members.is_empty()
            && native
                .tracked
                .iter()
                .all(|tracked| tracked.state == TrackedState::Absent))
    }
}

fn assigned(lease: &LeaseId, worker: WorkerId) -> anyhow::Result<ProviderDispatchPayload> {
    let target = DispatchTarget {
        worker_id: worker,
        attempt_id: DispatchAttemptId::new(format!("{lease}-attempt")).map_err(anyhow::Error::msg)?,
        epoch: Some(LeaseEpoch::new(2)?),
    };
    let mut payload = targeted_process_support::dispatch_payload(
        "docker-command-fence",
        lease,
        targeted_process_support::registry_image(),
        None,
        Some(target),
    )?;
    payload.request.persistent = LeasePersistence::Persistent;
    Ok(payload)
}

async fn rpc<T: Serialize, R: DeserializeOwned>(
    nats: &Nats,
    signer: &DispatchSigner,
    assignment: &ProviderDispatchPayload,
    subject: String,
    request: &T,
) -> anyhow::Result<R> {
    let target = assignment
        .target
        .as_ref()
        .context("public command requires actual assigned ownership")?;
    let bytes = serde_json::to_vec(request)?;
    let scope = VerbScope {
        tenant: assignment.request.tenant.clone(),
        lease_id: assignment.lease_id.clone(),
        worker_id: Some(target.worker_id.clone()),
        attempt_id: Some(target.attempt_id.clone()),
        epoch: target.epoch,
    };
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(AUTH_HEADER, signer.sign_verb(&subject, &bytes, &scope)?);
    let reply = nats
        .client
        .send_request(
            subject,
            async_nats::Request::new()
                .headers(headers)
                .payload(bytes.into())
                .timeout(Some(Duration::from_secs(20))),
        )
        .await?;
    serde_json::from_slice(&reply.payload)
        .context("actual signed worker reply did not match its public command contract")
}

fn subject(assignment: &ProviderDispatchPayload, verb: fn(ProviderKind, &LeaseId) -> String) -> String {
    verb(ProviderKind::LocalDocker, &assignment.lease_id)
}

async fn start(
    nats: &Nats,
    signer: &DispatchSigner,
    assignment: &ProviderDispatchPayload,
    family: &Family,
) -> anyhow::Result<RunCommandStartedResponse> {
    match rpc(
        nats,
        signer,
        assignment,
        subject(assignment, subjects::provider_run_command),
        &family.command,
    )
    .await?
    {
        RunCommandResponse::Started(started) => Ok(started),
        _ => {
            anyhow::bail!("intact actual assigned worker did not start its positive native command")
        }
    }
}

async fn cancel(
    nats: &Nats,
    signer: &DispatchSigner,
    assignment: &ProviderDispatchPayload,
    key: &LaunchKey,
) -> anyhow::Result<CancelCommandLaunchResponse> {
    rpc(
        nats,
        signer,
        assignment,
        subject(assignment, subjects::provider_cancel_command_launch),
        &CancelCommandLaunchRequest {
            lease_id: assignment.lease_id.clone(),
            launch_key: key.clone(),
        },
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires actual Docker, cached node:22, nats-server and SANDBOX_DOCKER_COMMAND_SUPERVISOR; run with --ignored"]
async fn replaced_fence_refuses_fresh_assigned_commands_and_restart_but_preserves_exact_old_cancellation()
-> anyhow::Result<()> {
    let supervisor = trogon_std::env::ReadEnv::var_os(&trogon_std::env::SystemEnv, "SANDBOX_DOCKER_COMMAND_SUPERVISOR")
        .context("set SANDBOX_DOCKER_COMMAND_SUPERVISOR to the verified immutable native artifact")?;
    let digest = Sha256::digest(std::fs::read(&supervisor)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let nats = Nats::start().await?;
    let directory = tempfile::tempdir()?;
    let state = directory.path().join("state");
    let signer = targeted_process_support::signer()?;
    let owner = format!("docker-command-fence-{}", fresh_id());
    let mut assets = NativeAssets::new(owner.clone()).await?;
    let docker = Docker::connect_with_defaults()?.with_timeout(Duration::from_secs(10));
    let image = docker
        .inspect_image(targeted_process_support::IMAGE)
        .await?
        .id
        .context("cached immutable Node22 image absent")?;
    let lease = LeaseId::new(format!("command-fence-{}", fresh_id()));
    assets.register_lease(lease.as_str());
    let mut heartbeats = nats
        .client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::LocalDocker))
        .await?;
    nats.client.flush().await?;
    let mut worker =
        targeted_process_support::worker_with_supervisor(&nats, &state, &owner, &signer, Some(Path::new(&supervisor)))?;
    let mut assignment = None;
    let mut grant: Option<ProviderLease> = None;
    let mut swap: Option<FenceSwap> = None;
    let mut families = Vec::new();
    let observed=std::panic::AssertUnwindSafe(async {
        let message=tokio::time::timeout(Duration::from_secs(15),heartbeats.next()).await?.context("actual configured worker heartbeat absent")?;
        let heartbeat:WorkerHeartbeat=serde_json::from_slice(&message.payload)?;
        ensure!(worker.is_alive()? && heartbeat.provider==ProviderKind::LocalDocker && heartbeat.dispatch_protocol==WorkerDispatchProtocol::TargetedV1 && heartbeat.supports_lease_epoch.is_supported(),"actual worker did not earn healthy signed assigned intake");
        let assignment:&ProviderDispatchPayload=assignment.insert(assigned(&lease,heartbeat.worker_id.clone())?);
        let target=assignment.target.as_ref().context("assigned dispatch lost its target")?;
        let body=signer.sign_payload(&serde_json::to_vec(assignment)?,OP_LEASE_DISPATCH).map_err(anyhow::Error::msg)?;
        let reply=nats.client.send_request(subjects::worker_dispatch(ProviderKind::LocalDocker,&target.worker_id),async_nats::Request::new().payload(body.into()).timeout(Some(Duration::from_secs(20)))).await?;
        let grant:&ProviderLease=grant.insert(match serde_json::from_slice(&reply.payload)? {ProviderDispatchReply::Granted(grant)=>grant,_=>anyhow::bail!("actual assigned worker refused its healthy native control")});
        assets.register_id(grant.sandbox_id.as_str());
        assets.running(grant.sandbox_id.as_str(),lease.as_str()).await?;
        ensure!(grant.worker_id.as_ref()==Some(&target.worker_id) && grant.attempt_id.as_ref()==Some(&target.attempt_id) && grant.epoch==target.epoch,"native grant changed its exact signed assignment");
        let ordinary:RunCommandRequest=serde_json::from_value(json!({"lease_id":lease,"command":["node","-e","process.stdout.write('ordinary-control')"]}))?;
        let response:RunCommandResponse=rpc(&nats,&signer,assignment,subject(assignment,subjects::provider_run_command),&ordinary).await?;
        ensure!(matches!(response,RunCommandResponse::Completed(ref output) if output.exit_code==0 && output.stdout==b"ordinary-control" && output.stderr.is_empty() && !output.timed_out.is_timed_out() && output.truncated==OutputTruncation::Complete),"healthy actual ordinary command positive failed");
        let inspected=docker.inspect_container(grant.sandbox_id.as_str(),None).await?;
        ensure!(inspected.mounts.as_ref().is_some_and(|mounts|mounts.iter().any(|mount|mount.rw==Some(false) && mount.name.as_ref().is_some_and(|name|name.starts_with(&format!("sandbox-command-{digest}-"))))),"actual positive workload did not mount its exact immutable native runtime readonly");
        let binding=Binding::seal_with_retained_capabilities(&docker,grant.sandbox_id.to_string(),lease.to_string(),owner.clone(),image.clone(),ContainerHardening::secure().capability_sets(&[]).0).await?;
        for (key,strong) in [("sentinel",false),("owned-old",true),("restart-old",true),("fresh-after-fault",true)] {
            families.push(Family::new(&docker,&binding,&image,key,strong).await?);
        }
        start(&nats,&signer,assignment,&families[0]).await?;
        let sentinel=families[0].ready(1).await?;
        let old=start(&nats,&signer,assignment,&families[1]).await?;
        let old_native=families[1].ready(1).await?;
        let restart_old=start(&nats,&signer,assignment,&families[2]).await?;
        let restart_before=families[2].ready(1).await?;
        let positive_restart:RestartCommandResponse=rpc(&nats,&signer,assignment,subject(assignment,subjects::provider_restart_command),&RestartCommandRequest {lease_id:lease.clone(),execution_id:restart_old.execution_id.clone(),grace_seconds:0}).await?;
        let restart_id=match positive_restart {RestartCommandResponse::Restarted {execution_id}=>execution_id,_=>anyhow::bail!("intact actual controlled restart positive failed")};
        ensure!(restart_id!=restart_old.execution_id,"positive restart reused old execution authority");
        let restart_native=families[2].ready(2).await?;
        let old_pins = Family::pins(&restart_before);
        let new_pins = Family::pins(&restart_native);
        let restarted = families[2].snapshot(&old_pins).await?;
        let old_absent = old_pins.iter().all(|identity| restarted.tracked.iter().any(|tracked|
            tracked.pid == identity.pid && tracked.start_time == identity.start_time && tracked.state == TrackedState::Absent));
        let new_alive = new_pins.iter().all(|identity| restarted.tracked.iter().any(|tracked|
            tracked.pid == identity.pid && tracked.start_time == identity.start_time && tracked.state == TrackedState::Alive));
        let disjoint = old_pins.iter().all(|identity| !new_pins.contains(identity));
        eprintln!("assigned Docker healthy restart witness: old_pinned_count={} new_pinned_count={} old_exact_absent={old_absent} new_exact_alive={new_alive} old_new_disjoint={disjoint}",old_pins.len(),new_pins.len());
        ensure!(disjoint && old_absent && new_alive && restarted.members == restart_native.members
            && restarted.starts == restart_native.starts,"healthy restart did not retire its exact old native family while preserving its exact new family");
        eprintln!("assigned Docker stale-fence controls: signed_targeted_epoch=true ordinary_positive=true immutable_readonly_runtime=true strong_owned_family=true independent_sentinel=true healthy_controlled_restart=true");
        let swap:&FenceSwap=swap.insert(FenceSwap::replace(&state.join("dispatch-fence.jsonl"))?);
        let fresh:RunCommandResponse=rpc(&nats,&signer,assignment,subject(assignment,subjects::provider_run_command),&families[3].command).await?;
        let fresh_native=if matches!(fresh,RunCommandResponse::Started(_)) {families[3].ready(1).await?} else {families[3].snapshot(&[]).await?};
        let restart:RestartCommandResponse=rpc(&nats,&signer,assignment,subject(assignment,subjects::provider_restart_command),&RestartCommandRequest {lease_id:lease.clone(),execution_id:restart_id,grace_seconds:0}).await?;
        let restart_after=if matches!(restart,RestartCommandResponse::Restarted {..}) {families[2].ready(3).await?} else {families[2].snapshot(&[]).await?};
        let canceled=cancel(&nats,&signer,assignment,&families[1].key).await?;
        let old_canceled=matches!(&canceled,CancelCommandLaunchResponse::Canceled {execution_id:Some(execution_id)} if execution_id==&old.execution_id);
        let old_absent=families[1].absent(&old_native).await?;
        let old_replayed=cancel(&nats,&signer,assignment,&families[1].key).await?==canceled;
        let sentinel_preserved=families[0].preserved(&sentinel).await?;
        let fault_present=swap.present()?;
        let fresh_refused=matches!(fresh,RunCommandResponse::Denied(_));
        let restart_refused=matches!(restart,RestartCommandResponse::Denied(_));
        let fresh_effect=!fresh_native.members.is_empty() || !fresh_native.starts.is_empty();
        let restart_effect=restart_after.starts.len()!=restart_native.starts.len();
        if let RunCommandResponse::Started(_) = fresh {
            ensure!(matches!(cancel(&nats,&signer,assignment,&families[3].key).await?,CancelCommandLaunchResponse::Canceled {..}) && families[3].absent(&fresh_native).await?,"unexpected fresh work did not retain exact product command cleanup");
        }
        if let RestartCommandResponse::Restarted {execution_id} = restart {
            let killed:KillCommandResponse=rpc(&nats,&signer,assignment,subject(assignment,subjects::provider_kill_command),&KillCommandRequest {lease_id:lease.clone(),execution_id,signal:UnixSignal::Kill}).await?;
            ensure!(killed==KillCommandResponse::Signaled && families[2].absent(&restart_after).await?,"unexpected restarted work did not retain exact product command cleanup");
        }
        ensure!(families[0].preserved(&sentinel).await? && swap.present()?,"semantic command cleanup changed sentinel or repaired the fault");
        Ok::<_,anyhow::Error>((fresh_refused,restart_refused,fresh_effect,restart_effect,old_canceled,old_absent,old_replayed,sentinel_preserved,fault_present))
    }).catch_unwind().await;
    let mut observer_cleanup = Ok(());
    for family in &families {
        if let Err(error) = family.observer.cleanup().await {
            observer_cleanup = Err(error);
        }
    }
    let restored = match swap.as_mut() {
        Some(swap) => swap.restore(),
        None => Ok(()),
    };
    let cleanup=async {
        if let (Some(assignment),Some(grant))=(&assignment,&grant) {
            let target=assignment.target.as_ref().context("assigned dispatch lost its target")?;
            let request=ProviderCleanupRequest {identity:LeaseAttemptIdentity {lease_id:lease.clone(),provider:ProviderKind::LocalDocker,target:target.clone()},intent:CancelIntent::Release};
            let response:ProviderCleanupResponse=rpc(&nats,&signer,assignment,subjects::worker_cleanup(ProviderKind::LocalDocker,&target.worker_id),&request).await?;
            ensure!(matches!(response,ProviderCleanupResponse::Cleaned {ref identity,intent:CancelIntent::Release} if identity==&request.identity),"physical fixture cleanup did not acknowledge exact assigned identity");
            assets.wait_absent(grant.sandbox_id.as_str()).await?;
        }
        Ok::<_,anyhow::Error>(())
    }.await;
    let stopped = worker.stop_gracefully().await;
    if stopped.is_err() {
        worker.stop()?;
    }
    let fallback = assets.finish().await?;
    let runtime_fallback =
        trogon_sandbox_docker_worker::adapter::command_runtime::CommandRuntime::sweep_owned(&docker, &owner).await?;
    restored?;
    cleanup?;
    stopped?;
    observer_cleanup?;
    ensure!(
        !fallback && runtime_fallback == 0,
        "physical fixture cleanup required native/runtime fallback"
    );
    let (
        fresh_refused,
        restart_refused,
        fresh_effect,
        restart_effect,
        old_canceled,
        old_absent,
        old_replayed,
        sentinel_preserved,
        fault_present,
    ) = match observed {
        Ok(result) => result?,
        Err(_) => anyhow::bail!("assigned command fence lane panicked after exact cleanup"),
    };
    eprintln!(
        "assigned Docker stale-fence receipt: ordinary_positive=true immutable_readonly_runtime_positive=true healthy_controlled_restart_positive=true fault_present_during_semantics={fault_present} old_exact_canceled={old_canceled} old_exact_absent={old_absent} old_receipt_replayed={old_replayed} sentinel_preserved={sentinel_preserved} fresh_refused={fresh_refused} fresh_native_effect={fresh_effect} restart_refused={restart_refused} restart_native_effect={restart_effect} original_inode_restored_for_physical_cleanup_only=true exact_container_runtime_cleanup=true fallback_used=false"
    );
    ensure!(
        fault_present && old_canceled && old_absent && old_replayed && sentinel_preserved,
        "durable fence health loss did not preserve old exact command authority and independent sentinel"
    );
    ensure!(
        fresh_refused && !fresh_effect,
        "unhealthy durable fence must refuse fresh assigned command effects"
    );
    ensure!(
        restart_refused && !restart_effect,
        "unhealthy durable fence must refuse fresh controlled restart effects"
    );
    Ok(())
}
