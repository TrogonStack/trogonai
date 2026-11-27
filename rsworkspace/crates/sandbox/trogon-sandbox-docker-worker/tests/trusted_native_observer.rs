#![cfg(unix)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, ensure};
use bollard::Docker;
use bollard::exec::{CreateExecOptions, StartExecOptions};
use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder, RestartContainerOptionsBuilder,
};
use futures::FutureExt;
use serde_json::Value;
use sha2::{Digest, Sha256};

mod trusted_observer_support;
use trusted_observer_support::{
    Binding, Identity, Native, Observer, Operation, Proof, Refusal, Request, Response, Rpc, TrackedState, absent,
    capture, fresh_id,
};

const PYTHON: &str = "python:3.12-slim";
const NODE: &str = "docker.io/library/node:22-bookworm-slim";
const OWNER: &str = "sandbox.trogon.live/owner";
const LEASE: &str = "sandbox.trogon.live/lease-id";

const FAMILY: &str = r#"import ctypes,json,os,signal,sys,time
root,marker=sys.argv[1:3]
os.makedirs(root,exist_ok=True)
base=root+'/'+marker
def record(pid,path):
 value=open('/proc/'+str(pid)+'/stat').read();fields=value[value.rfind(')')+1:].split()
 data={'pid':pid,'startTime':fields[19],'marker':marker}
 tmp=path+'.tmp';open(tmp,'w').write(json.dumps(data));os.rename(tmp,path)
libc=ctypes.CDLL(None,use_errno=True)
signal.signal(signal.SIGTERM,signal.SIG_IGN)
if libc.prctl(4,0,0,0,0)!=0:raise RuntimeError('nondumpable_control_failed')
child=os.fork()
if child==0:
 if libc.prctl(4,1,0,0,0)!=0:os._exit(43)
 record(os.getpid(),base+'.child.json')
 while True:time.sleep(.05)
def stop(signum,frame):
 os.kill(child,signal.SIGKILL);os.waitpid(child,0);os._exit(0)
signal.signal(signal.SIGUSR1,stop)
open(base+'.starts','a').write(marker+chr(10))
record(os.getpid(),base+'.root.json')
while True:time.sleep(.05)
"#;

fn nonce(marker: &str) -> anyhow::Result<&str> {
    marker
        .split_once('-')
        .map(|(_, nonce)| nonce)
        .context("family marker has no nonce")
}

struct Workload {
    docker: Docker,
    id: String,
    owner: String,
    lease: String,
    image: String,
}

impl Workload {
    async fn create(docker: Docker) -> anyhow::Result<Self> {
        let image = docker
            .inspect_image(PYTHON)
            .await?
            .id
            .context("cached immutable Python image required")?;
        let owner = format!("observer-preflight-{}", fresh_id());
        let lease = "observer-preflight-lease".to_string();
        let created = docker
            .create_container(
                Some(CreateContainerOptionsBuilder::new().name(&owner).build()),
                ContainerCreateBody {
                    image: Some(image.clone()),
                    user: Some("0".into()),
                    entrypoint: Some(vec!["python".into()]),
                    cmd: Some(vec!["-c".into(), "import time\nwhile True:time.sleep(1)".into()]),
                    labels: Some(HashMap::from([
                        (OWNER.into(), owner.clone()),
                        (LEASE.into(), lease.clone()),
                    ])),
                    host_config: Some(HostConfig {
                        cap_drop: Some(vec!["ALL".into()]),
                        network_mode: Some("none".into()),
                        security_opt: Some(vec!["no-new-privileges:true".into()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        let workload = Self {
            docker,
            id: created.id,
            owner,
            lease,
            image,
        };
        if let Err(error) = workload.docker.start_container(&workload.id, None).await {
            workload.cleanup().await?;
            return Err(error.into());
        }
        Ok(workload)
    }

    fn request(&self, marker: &str, tracked: Vec<Identity>) -> anyhow::Result<Request> {
        let nonce = nonce(marker)?;
        let base = format!("/tmp/named-cancellation-{nonce}/{marker}");
        Ok(Request {
            kind: Operation::NativeFamily,
            request_id: fresh_id(),
            lease_id: self.lease.clone(),
            container_id: self.id.clone(),
            marker: marker.into(),
            paths: [
                format!("{base}.root.json"),
                format!("{base}.child.json"),
                format!("{base}.starts"),
            ],
            tracked,
        })
    }

    async fn family(&self, marker: &str) -> anyhow::Result<()> {
        let nonce = nonce(marker)?;
        let exec = self
            .docker
            .create_exec(
                &self.id,
                CreateExecOptions {
                    cmd: Some(vec![
                        "python".to_string(),
                        "-c".into(),
                        FAMILY.into(),
                        format!("/tmp/named-cancellation-{nonce}"),
                        marker.into(),
                    ]),
                    env: Some(vec![format!("SANDBOX_ACCEPTANCE_FAMILY={marker}")]),
                    user: Some("0".into()),
                    privileged: Some(false),
                    ..Default::default()
                },
            )
            .await?;
        self.docker
            .start_exec(
                &exec.id,
                Some(StartExecOptions {
                    detach: true,
                    ..Default::default()
                }),
            )
            .await?;
        Ok(())
    }

    async fn cleanup(&self) -> anyhow::Result<()> {
        if absent(&self.docker, &self.id).await? {
            return Ok(());
        }
        let found = self.docker.inspect_container(&self.id, None).await?;
        ensure!(
            found.id.as_deref() == Some(self.id.as_str())
                && found
                    .config
                    .as_ref()
                    .and_then(|config| config.labels.as_ref())
                    .is_some_and(
                        |labels| labels.get(OWNER) == Some(&self.owner) && labels.get(LEASE) == Some(&self.lease)
                    ),
            "preflight cleanup ownership changed"
        );
        self.docker
            .remove_container(
                &self.id,
                Some(RemoveContainerOptionsBuilder::new().force(true).v(true).build()),
            )
            .await?;
        ensure!(
            absent(&self.docker, &self.id).await?,
            "exact preflight workload absence unconfirmed"
        );
        Ok(())
    }
}

async fn observed(rpc: &Rpc, request: &Request) -> anyhow::Result<(Native, Proof, String)> {
    match rpc.call(request).await? {
        Response::Observed {
            native,
            proof,
            observer_container,
            observer_absent,
            ..
        } => {
            ensure!(observer_absent, "observer response preceded exact teardown");
            Ok((native, proof, observer_container))
        }
        Response::Refused { code, .. } => {
            anyhow::bail!("readonly observer refused its bound request: {code:?}")
        }
        Response::Failed { code, .. } => {
            anyhow::bail!("readonly observer failed strictly: {code:?}")
        }
    }
}

async fn ready(rpc: &Rpc, request: &Request) -> anyhow::Result<(Native, Proof, String)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let result = observed(rpc, request).await?;
        if result.0.ready {
            return Ok(result);
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "original native family readiness budget exceeded"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn live(native: &Native, proof: &Proof, marker: &str) -> anyhow::Result<()> {
    ensure!(
        native.ready
            && native.records.len() == 2
            && native.members.len() == 2
            && native.records[0].pid != native.records[1].pid,
        "exact protected root/child census missing"
    );
    ensure!(
        proof.root_child_linked && proof.ptrace && proof.uid == 0,
        "kernel parent/start or trusted reader privilege unproven"
    );
    ensure!(
        native.records.iter().all(|record| record.marker == marker
            && native
                .members
                .iter()
                .any(|member| member.pid == record.pid && member.start_time == record.start_time)),
        "marker records do not match complete kernel census"
    );
    ensure!(
        native.tracked.len() == 2
            && native
                .tracked
                .iter()
                .all(|identity| identity.state == TrackedState::Alive),
        "exact family PID/start pins not alive"
    );
    ensure!(
        native.starts == vec![marker.to_string()],
        "actual user effect was replayed"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_TRUSTED_OBSERVER=1, actual Unix Docker, cached python:3.12-slim and node:22 images; run with --ignored"]
async fn trusted_readonly_observer_kernel_rpc_and_namespace_preflight() -> anyhow::Result<()> {
    ensure!(
        trogon_std::env::ReadEnv::var(&trogon_std::env::SystemEnv, "SANDBOX_DOCKER_TRUSTED_OBSERVER").as_deref()
            == Ok("1"),
        "set SANDBOX_DOCKER_TRUSTED_OBSERVER=1 for owned native fixture preflight"
    );
    let docker = Docker::connect_with_defaults()?.with_timeout(Duration::from_secs(20));
    let node = docker
        .inspect_image(NODE)
        .await?
        .id
        .context("cached immutable Node observer image required")?;
    let workload = Workload::create(docker.clone()).await?;
    let mut observer: Option<Arc<Observer>> = None;
    let mut rpc: Option<Rpc> = None;
    let operation=std::panic::AssertUnwindSafe(async {
        let binding=Binding::seal(&docker,workload.id.clone(),workload.lease.clone(),workload.owner.clone(),workload.image.clone()).await?;
        let reader=Observer::create(docker.clone(),binding,node,format!("readonly-reader-{}",fresh_id())).await?;
        observer=Some(reader.clone());
        let rpc:&Rpc=rpc.insert(Rpc::start(reader.clone()).await?);
        let nonce=uuid::Uuid::now_v7().simple().to_string();
        let sentinel=format!("sentinel-{nonce}");
        let owned=format!("owned-{nonce}");
        let request=workload.request(&sentinel,vec![])?;
        let mut foreign=request.clone();foreign.container_id="f".repeat(64);
        ensure!(matches!(rpc.call(&foreign).await?,Response::Refused {code:Refusal::RequestScope,..}),"foreign CID acquired observer authority");
        foreign=request.clone();foreign.lease_id="foreign-lease".into();
        ensure!(matches!(rpc.call(&foreign).await?,Response::Refused {code:Refusal::RequestScope,..}),"foreign lease acquired observer authority");
        foreign=request.clone();foreign.paths[0]="/proc/1/environ".into();
        ensure!(matches!(rpc.call(&foreign).await?,Response::Refused {code:Refusal::RequestScope,..}),"forged path acquired observer authority");
        let mut unknown=serde_json::to_value(&request)?;unknown["script"]="arbitrary code".into();
        ensure!(matches!(rpc.call_bytes(&serde_json::to_vec(&unknown)?).await?,Response::Refused {code:Refusal::InvalidInput,..}),"unknown command input was accepted");
        ensure!(reader.executions()==0 && reader.containers().await.is_empty(),"refused inputs caused native observer execution");
        workload.family(&sentinel).await?;
        workload.family(&owned).await?;
        let (before,sentinel_proof,sentinel_observer)=ready(rpc,&request).await?;
        live(&before,&sentinel_proof,&sentinel)?;
        ensure!(absent(&docker,&sentinel_observer).await?,"trusted observer remains visible after its RPC");
        let check=r#"import json,sys
pid,start=sys.argv[1:3]
try:
 value=open('/proc/'+pid+'/stat').read();actual=value[value.rfind(')')+1:].split()[19];absent=actual!=start
except FileNotFoundError:absent=True
print(json.dumps({'observer_absent':absent}))
"#;
        let control=capture(&docker,&workload.id,vec!["python".into(),"-c".into(),check.into(),sentinel_proof.observer.pid.to_string(),sentinel_proof.observer.start_time.clone()],false).await?;
        ensure!(control.exit==0 && serde_json::from_slice::<Value>(&control.stdout)?["observer_absent"]==true,"observer PID/start contaminated a subsequent unprivileged workload command");
        let denied=r#"import errno,json,os
denied=False
for pid in os.listdir('/proc'):
 if not pid.isdigit():continue
 try:open('/proc/'+pid+'/environ','rb').read()
 except OSError as error:
  if error.errno in (errno.ENOENT,errno.ESRCH):continue
  if error.errno==errno.EACCES:denied=True;break
  raise
print(json.dumps({'protected_read_denied':denied}))
"#;
        let control=capture(&docker,&workload.id,vec!["python".into(),"-c".into(),denied.into()],false).await?;
        ensure!(control.exit==0 && serde_json::from_slice::<Value>(&control.stdout)?["protected_read_denied"]==true,"protected kernel countercontrol did not establish actual EACCES");
        let target=workload.request(&owned,vec![])?;
        let (owned_before,proof,observer_id)=ready(rpc,&target).await?;
        live(&owned_before,&proof,&owned)?;
        ensure!(absent(&docker,&observer_id).await?,"unadmitted protected process was not counted by a fully torn-down observer");
        let stop=r#"import json,os,signal,sys
pin=json.loads(sys.argv[1]);value=open('/proc/'+str(pin['pid'])+'/stat').read();start=value[value.rfind(')')+1:].split()[19]
if start!=pin['startTime']:raise RuntimeError('family_pin_changed')
os.kill(pin['pid'],signal.SIGUSR1)
print('stopped')
"#;
        let root=&owned_before.records[0];
        let control=capture(&docker,&workload.id,vec!["python".into(),"-c".into(),stop.into(),serde_json::json!({"pid":root.pid,"startTime":root.start_time}).to_string()],false).await?;
        ensure!(control.exit==0,"exact preflight family stop failed");
        let deadline=tokio::time::Instant::now()+Duration::from_secs(10);
        loop {
            let (state,_,_)=observed(rpc,&workload.request(&owned,owned_before.members.clone())?).await?;
            ensure!(state.starts==owned_before.starts,"absent-family observation replayed user effects");
            if state.members.is_empty() && state.tracked.iter().all(|identity|identity.state==TrackedState::Absent) {break;}
            ensure!(tokio::time::Instant::now()<deadline,"original exact absence budget exceeded");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let (after,proof,_)=observed(rpc,&workload.request(&sentinel,before.members.clone())?).await?;
        live(&after,&proof,&sentinel)?;
        let mut left=before.members.clone();let mut right=after.members.clone();
        left.sort_by_key(|identity|identity.pid);right.sort_by_key(|identity|identity.pid);
        ensure!(left==right && before.starts==after.starts,"unrelated sentinel family changed");
        let executions=reader.executions();let containers=reader.containers().await.len();
        docker.restart_container(&workload.id,Some(RestartContainerOptionsBuilder::new().t(1).build())).await?;
        ensure!(matches!(rpc.call(&request).await?,Response::Refused {code:Refusal::NamespaceChanged,..}),"same-CID restart reused an old namespace emptiness oracle");
        ensure!(reader.executions()==executions && reader.containers().await.len()==containers,"namespace refusal created observer effects");
        for id in reader.containers().await {ensure!(absent(&docker,&id).await?,"per-probe observer container remains after a response");}
        let asset_sha=Sha256::digest(trusted_observer_support::PROBE.as_bytes()).iter().map(|byte|format!("{byte:02x}")).collect::<String>();
        eprintln!("trusted readonly observer preflight: input_refusals=true protected_eacces_control=true protected_unknown_marker_counted=true exact_root_child_pins=true no_replay=true exact_family_absence=true sentinel_preserved=true namespace_restart_refused=true observers_absent_before_reply=true probe_sha256={asset_sha}");
        Ok::<_,anyhow::Error>(())
    }).catch_unwind().await;
    let rpc_cleanup = if let Some(rpc) = rpc.as_mut() {
        rpc.shutdown().await
    } else {
        Ok(())
    };
    let observer_cleanup = if let Some(observer) = observer.as_ref() {
        observer.cleanup().await
    } else {
        Ok(())
    };
    let workload_cleanup = workload.cleanup().await;
    rpc_cleanup?;
    observer_cleanup?;
    workload_cleanup?;
    if let Some(observer) = observer {
        for id in observer.containers().await {
            ensure!(absent(&docker, &id).await?, "exact observer cleanup unconfirmed");
        }
    }
    eprintln!(
        "trusted readonly observer cleanup: exact_observers_absent=true exact_workload_absent=true product_capabilities_unchanged=true product_acceptance=false"
    );
    match operation {
        Ok(result) => result,
        Err(_) => anyhow::bail!("trusted observer preflight panicked after exact cleanup"),
    }
}
