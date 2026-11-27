//! Strong named launches on a supervised Docker lease: exact native cancellation, preserved reuse, refused admission
//! and custody that never outlives its native registration.

mod fixture;

use std::io::Read as _;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, ensure};
use bollard::query_parameters::DownloadFromContainerOptionsBuilder;
use sandbox_command_supervisor::protocol::{
    self as supervisor, Cancellation, CommandWire, Delivery, EventWire, ReleaseState,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest as _;
use tokio::io::AsyncWriteExt as _;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    CommandEnv, CommandLaunchOrigin, DispatchAttemptId, ExtraCapability, KillCommandResponse, LaunchCancellation,
    LaunchKey, LeaseEpoch, RestartCommandResponse, RunCommandStartedResponse, SandboxUserName, StopCommandResponse,
    StopSignal, StreamedOutput, TenantId, UnixSignal, WorkerId,
};
use trogon_sandbox_protocol::{
    AUTH_HEADER, DEFAULT_MAX_OUTPUT_BYTES, DispatchSigner, DispatchVerifier, EnforcementMode, VerbScope,
    subscribe_with_context,
};
use trogon_sandbox_providers::{ContainerHardening, LocalDockerProvider};
use trogon_std::env::{ReadEnv, SystemEnv};

use super::docker_lease::{NODE_IMAGE, docker_provider_over, node_lease_request_from, node_spec_parts};
use super::*;
use crate::adapter::command_runtime::CommandRuntime;
use crate::fresh_id::{fresh_id, is_fresh_id};

/// The JetStream key-value bucket the worker keeps command custody records in.
const CUSTODY_BUCKET: &str = "sandbox-docker-command-custody";
/// The label naming the worker that owns a Docker object.
const OWNER_LABEL: &str = "sandbox.trogon.live/owner";
/// The label pinning a runtime volume to the digest of the supervisor it holds.
const RUNTIME_DIGEST_LABEL: &str = "sandbox.trogon.live/command-runtime-digest";

fn native_output_class(bytes: &[u8]) -> &'static str {
    let Ok(value) = std::str::from_utf8(bytes) else {
        return "non_utf8";
    };
    for (needle, class) in [
        ("SyntaxError", "syntax_error"),
        ("ReferenceError", "reference_error"),
        ("TypeError", "type_error"),
        ("EACCES", "access_denied"),
        ("EPERM", "permission_denied"),
        ("ENOENT", "missing_file"),
        ("ESRCH", "missing_process"),
        ("census bound", "census_bound"),
        ("command timed out", "command_timeout"),
    ] {
        if value.contains(needle) {
            return class;
        }
    }
    if value.is_empty() { "empty" } else { "other" }
}

fn log_owned_container(grant: &ProviderLease) {
    let id = grant.sandbox_id.as_str();
    if id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        eprintln!("native fixture allocation: owned_container_id={id}");
    } else {
        eprintln!("native fixture allocation: owned_container_id_class=noncanonical");
    }
}

async fn cleanup_owned_container(
    provider: &DockerWorkerProvider,
    lease: &LeaseId,
    grant: &ProviderLease,
) -> anyhow::Result<()> {
    log_owned_container(grant);
    let cleanup = tokio::time::timeout(Duration::from_secs(25), provider.cancel(lease)).await;
    let docker = bollard::Docker::connect_with_defaults()?;
    let absence = tokio::time::timeout(
        Duration::from_secs(5),
        docker.inspect_container(grant.sandbox_id.as_str(), None),
    )
    .await;
    let native_absent = matches!(
        absence,
        Ok(Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404,
            ..
        }))
    );
    eprintln!(
        "native fixture cleanup: provider_cleanup_confirmed={} exact_container_absent={} fallback_used=false",
        matches!(cleanup, Ok(Ok(()))),
        native_absent
    );
    cleanup??;
    ensure!(native_absent, "exact owned native container absence was not confirmed");
    Ok(())
}

struct Fixture {
    _server: JetStreamTestServer,
    client: Client,
    js: jetstream::Context,
    provider: Arc<DockerWorkerProvider>,
    lease: LeaseId,
    grant: ProviderLease,
    payload: ProviderDispatchPayload,
    signer: DispatchSigner,
    scope: VerbScope,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    active: ActiveLeases,
    anchors: Mutex<Vec<Identity>>,
    worker: String,
    runtime_digest: String,
    runtime_volume: String,
    runtime_bytes: Vec<u8>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
struct Identity {
    pid: u32,
    start: String,
}

#[derive(Deserialize)]
struct Native {
    records: Vec<Identity>,
    members: Vec<Identity>,
    tracked_alive: usize,
    starts: Vec<String>,
    anchors: Vec<Identity>,
}

#[derive(Debug, Deserialize)]
enum DeniedAnchorClass {
    #[serde(rename = "retained_pid")]
    Retained,
    #[serde(rename = "candidate_pid")]
    Candidate,
    #[serde(rename = "unknown_pid")]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CensusPermissionDiagnostic {
    kind: String,
    code: String,
    pid: u32,
    start: Option<String>,
    anchor_class: DeniedAnchorClass,
    retained: Vec<Identity>,
    candidates: Vec<Identity>,
    verified: Vec<Identity>,
    retained_count: usize,
    candidate_count: usize,
}

impl std::fmt::Display for CensusPermissionDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "native census EACCES pid={} start={:?} class={:?}",
            self.pid, self.start, self.anchor_class
        )
    }
}

fn valid_start_time(start: &str) -> bool {
    !start.is_empty() && start.len() <= 20 && start.bytes().all(|byte| byte.is_ascii_digit())
}

fn valid_identity(identity: &Identity) -> bool {
    identity.pid > 0 && valid_start_time(&identity.start)
}

fn census_permission_diagnostic(stderr: &[u8]) -> Option<CensusPermissionDiagnostic> {
    let line = stderr.split(|byte| *byte == b'\n').next()?;
    if line.len() > 4096 {
        return None;
    }
    let diagnostic: CensusPermissionDiagnostic = serde_json::from_slice(line).ok()?;
    (diagnostic.kind == "census_permission_denied"
        && diagnostic.code == "EACCES"
        && diagnostic.pid > 0
        && diagnostic.start.as_deref().is_none_or(valid_start_time)
        && diagnostic.retained_count <= 4096
        && diagnostic.candidate_count <= 4096
        && [&diagnostic.retained, &diagnostic.candidates, &diagnostic.verified]
            .iter()
            .all(|identities| identities.len() <= 8 && identities.iter().all(valid_identity)))
    .then_some(diagnostic)
}

/// The node family a strong launch runs: a TERM-ignoring root and child that record their identities under `/tmp`.
const FAMILY: &str = r#"const fs=require('node:fs'),cp=require('node:child_process'),m=process.argv[1],d='/tmp/'+m;process.on('SIGTERM',()=>{});const child="const fs=require('node:fs');process.on('SIGTERM',()=>{});const v=fs.readFileSync('/proc/self/stat','utf8');fs.writeFileSync(process.argv[1]+'.tmp',JSON.stringify({pid:process.pid,start:v.slice(v.lastIndexOf(')')+1).trim().split(/\\s+/)[19]}));fs.renameSync(process.argv[1]+'.tmp',process.argv[1]);setInterval(()=>{},1000);";cp.spawn(process.execPath,['-e',child,d+'.child'],{stdio:'ignore'});const v=fs.readFileSync('/proc/self/stat','utf8');fs.appendFileSync(d+'.starts',m+'\n');fs.writeFileSync(d+'.root.tmp',JSON.stringify({pid:process.pid,start:v.slice(v.lastIndexOf(')')+1).trim().split(/\s+/)[19]}));fs.renameSync(d+'.root.tmp',d+'.root');setInterval(()=>{},1000);"#;
/// The census a lane runs inside the lease: the family members for a marker, its records and the live anchors.
const PROBE: &str = r#"const fs=require('node:fs'),m=process.argv[1],d='/tmp/'+m,pinned=JSON.parse(process.argv[2]),retained=JSON.parse(process.argv[3]),candidates=JSON.parse(process.argv[4]);const stat=p=>{try{const v=fs.readFileSync('/proc/'+p+'/stat','utf8'),fields=v.slice(v.lastIndexOf(')')+1).trim().split(/\s+/);return{pid:Number(p),start:fields[19],parent:Number(fields[1])};}catch(e){if(e.code==='ENOENT'||e.code==='ESRCH')return;throw e;}};const anchors=retained.filter(a=>stat(a.pid)?.start===a.start);for(const c of candidates){const h=stat(c.helper.pid);if(!h)continue;if(h.start!==c.helper.start)throw Error('anchor identity changed');if(anchors.some(a=>a.pid===h.pid&&a.start===h.start))continue;const r=stat(c.root.pid);if(!r||r.start!==c.root.start||r.parent!==h.pid)throw Error('anchor ancestry unconfirmed');anchors.push(c.helper);}const records=['.root','.child'].filter(s=>fs.existsSync(d+s)).map(s=>JSON.parse(fs.readFileSync(d+s,'utf8')));const pids=fs.readdirSync('/proc').filter(p=>/^\d+$/.test(p));if(pids.length>4096)throw Error('census bound');const members=[];for(const p of pids){try{if(fs.readFileSync('/proc/'+p+'/environ').toString().split('\0').includes('SANDBOX_ACCEPTANCE_FAMILY='+m)){const v=stat(p);if(v)members.push({pid:v.pid,start:v.start});}}catch(e){if(e.code==='ENOENT'||e.code==='ESRCH')continue;if(e.code==='EACCES'&&anchors.some(a=>a.pid===Number(p)&&stat(p)?.start===a.start))continue;if(e.code==='EACCES'){const identity=stat(p),samePid=value=>value.pid===Number(p),capturedRetained=retained.filter(samePid).slice(0,8),capturedCandidates=candidates.map(value=>value.helper).filter(samePid).slice(0,8),capturedVerified=anchors.filter(samePid).slice(0,8);fs.writeSync(2,JSON.stringify({kind:'census_permission_denied',code:'EACCES',pid:Number(p),start:identity?.start??null,anchor_class:capturedRetained.length?'retained_pid':capturedCandidates.length?'candidate_pid':'unknown_pid',retained:capturedRetained,candidates:capturedCandidates,verified:capturedVerified,retained_count:retained.length,candidate_count:candidates.length})+'\n');process.exit(1);}throw e;}}const tracked_alive=pinned.filter(p=>stat(p.pid)?.start===p.start).length,starts=fs.existsSync(d+'.starts')?fs.readFileSync(d+'.starts','utf8').trim().split('\n').filter(Boolean):[];process.stdout.write(JSON.stringify({records,members,tracked_alive,starts,anchors}));"#;
/// A family whose child escapes into a new session with a cleared environment.
const ESCAPED_FAMILY: &str = r#"const fs=require('node:fs'),cp=require('node:child_process'),d='/tmp/'+process.argv[1]+'.escaped';const child="const fs=require('node:fs');const v=fs.readFileSync('/proc/self/stat','utf8');fs.writeFileSync(process.argv[1]+'.tmp',JSON.stringify({pid:process.pid,start:v.slice(v.lastIndexOf(')')+1).trim().split(/\\s+/)[19]}));fs.renameSync(process.argv[1]+'.tmp',process.argv[1]);process.on('SIGTERM',()=>{});setInterval(()=>{},1000);";const p=cp.spawn(process.execPath,['-e',child,d],{detached:true,stdio:'ignore',env:{}});p.unref();const timer=setInterval(()=>{if(fs.existsSync(d)){clearInterval(timer);process.exit(0);}},10);"#;
/// Reads the escaped child identity for a marker and whether that exact process is still alive.
const ESCAPED_IDENTITY: &str = r#"const fs=require('node:fs'),d='/tmp/'+process.argv[1]+'.escaped';if(!fs.existsSync(d)){process.stdout.write('null');process.exit(0);}const identity=JSON.parse(fs.readFileSync(d,'utf8'));let alive=false;try{const v=fs.readFileSync('/proc/'+identity.pid+'/stat','utf8');alive=v.slice(v.lastIndexOf(')')+1).trim().split(/\s+/)[19]===identity.start;}catch(e){if(e.code!=='ENOENT'&&e.code!=='ESRCH')throw e;}process.stdout.write(JSON.stringify([identity,alive]));"#;
/// Reports the uid and whether CAP_SYS_PTRACE is effective.
const CAPABILITY_PROBE: &str = "const fs=require('node:fs'),status=fs.readFileSync('/proc/self/status','utf8');process.stdout.write(JSON.stringify({uid:process.getuid(),ptrace:(BigInt('0x'+status.match(/^CapEff:\\s*([0-9a-f]+)/m)[1])&(1n<<19n))!==0n}));";
/// Reports, for each pinned identity, whether that exact process is still alive.
const ALIVE_PROBE: &str = "const fs=require('node:fs'),values=JSON.parse(process.argv[1]);const alive=value=>{try{const stat=fs.readFileSync('/proc/'+value.pid+'/stat','utf8');return stat.slice(stat.lastIndexOf(')')+1).trim().split(/\\s+/)[19]===value.start;}catch(error){if(error.code==='ENOENT'||error.code==='ESRCH')return false;throw error;}};process.stdout.write(JSON.stringify(values.map(alive)));";

/// A launch that keeps its family but needs no launch cancellation.
fn ordinary(mut request: RunCommandRequest) -> RunCommandRequest {
    request.require_launch_cancellation = LaunchCancellation::NotRequired;
    request.launch_key = None;
    request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker, cached node:22-bookworm-slim and NATS; run with --ignored"]
async fn controlled_term_stop_completion_and_fresh_restart_preserve_public_contracts() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(true).await?;
    let observed = async {
        let marker = format!("restart-{}", fresh_id());
        let request = fixture.request("restartable", "restartable-key", &marker)?;
        let first = fixture.start(&request).await?;
        let before = fixture.ready(&marker).await?;
        ensure!(
            fixture
                .provider
                .kill_command_for_lease(&fixture.lease, &first.execution_id, UnixSignal::Term)
                .await
                == KillCommandResponse::Signaled,
            "controlled TERM no longer uses the public signal contract"
        );
        fixture.preserved(&marker, &before).await?;
        ensure!(
            matches!(
                fixture
                    .provider
                    .stop_command_for_lease(&fixture.lease, &first.execution_id, Duration::from_millis(100))
                    .await,
                StopCommandResponse::Stopped { signal: StopSignal::Kill }
            ),
            "graceful stop did not escalate for TERM-ignoring native family"
        );
        fixture.absent(&marker, &before).await?;
        ensure!(
            fixture
                .provider
                .kill_command_for_lease(&fixture.lease, &first.execution_id, UnixSignal::Term)
                .await
                == KillCommandResponse::AlreadyCompleted,
            "completed controlled TERM did not remain a no-op"
        );
        ensure!(
            fixture
                .provider
                .stop_command_for_lease(&fixture.lease, &first.execution_id, Duration::from_millis(100))
                .await
                == StopCommandResponse::AlreadyCompleted,
            "completed controlled stop did not remain a no-op"
        );
        let restarted = fixture
            .provider
            .restart_command_for_lease(&fixture.lease, &first.execution_id, Duration::from_millis(100))
            .await;
        let RestartCommandResponse::Restarted { execution_id } = restarted else {
            anyhow::bail!("controlled restart failed: {restarted:?}");
        };
        ensure!(execution_id != first.execution_id, "restart reused old execution authority");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let restarted_native = loop {
            let state = fixture.native(&marker, &[]).await?;
            if state.starts.len() == 2
                && state.records.len() == 2
                && state.records.iter().all(|record| state.members.contains(record))
            {
                break state;
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "restart did not execute the original user command"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        ensure!(
            restarted_native.starts == vec![marker.clone(), marker.clone()],
            "restart changed or duplicated original user work"
        );
        ensure!(
            matches!(
                fixture.cancel("restartable-key").await?,
                CancelCommandLaunchResponse::Canceled { ref execution_id } if execution_id.as_ref() == Some(&first.execution_id)
            ),
            "old launch authority moved to the restarted execution"
        );
        fixture.preserved(&marker, &restarted_native).await?;
        ensure!(
            fixture
                .provider
                .kill_command_for_lease(&fixture.lease, &execution_id, UnixSignal::Kill)
                .await
                == KillCommandResponse::Signaled,
            "fresh restarted execution lacks exact acknowledged cleanup"
        );
        fixture.absent(&marker, &restarted_native).await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    fixture.cleanup().await?;
    observed
}

async fn escaped_identity(fixture: &Fixture, marker: &str) -> anyhow::Result<Option<(Identity, bool)>> {
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        fixture.provider.run_command_for_lease(
            &fixture.lease,
            vec!["node".into(), "-e".into(), ESCAPED_IDENTITY.into(), marker.into()],
            None,
            &CommandEnv::default(),
            5,
            None,
            StreamedOutput::Disabled,
            DEFAULT_MAX_OUTPUT_BYTES,
        ),
    )
    .await??;
    ensure!(
        output.exit_code == 0,
        "escaped native identity probe failed: exit={} timed_out={:?} truncated={:?} stdout_bytes={} stderr_bytes={} stderr_class={}",
        output.exit_code,
        output.timed_out,
        output.truncated,
        output.stdout.len(),
        output.stderr.len(),
        native_output_class(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker, cached node:22-bookworm-slim and NATS; run with --ignored"]
async fn escaped_session_with_cleared_environment_cannot_survive_settled_cancellation() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(true).await?;
    let observed = async {
        let sentinel_marker = format!("escape-sentinel-{}", fresh_id());
        let sentinel = ordinary(fixture.request("escape-sentinel", "unused-sentinel-key", &sentinel_marker)?);
        fixture.start(&sentinel).await?;
        let sentinel_before = fixture.ready(&sentinel_marker).await?;
        let marker = format!("escaped-{}", fresh_id());
        let mut request = fixture.request("escaped", "escaped-owned-key", &marker)?;
        request.command = vec!["node".into(), "-e".into(), ESCAPED_FAMILY.into(), marker.clone()];
        let started = fixture.start(&request).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let escaped = loop {
            if let Some((identity, true)) = escaped_identity(&fixture, &marker).await? {
                break identity;
            }
            ensure!(
                tokio::time::Instant::now() < deadline,
                "escaped new-session child did not actually start"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        let census = fixture.native(&marker, std::slice::from_ref(&escaped)).await?;
        ensure!(
            census.tracked_alive == 1 && !census.members.contains(&escaped),
            "control did not establish a live descendant with cleared marker"
        );
        let canceled = fixture.cancel("escaped-owned-key").await?;
        ensure!(
            matches!(canceled, CancelCommandLaunchResponse::Canceled { ref execution_id } if execution_id.as_ref() == Some(&started.execution_id)),
            "owned escaped-family cancellation did not settle"
        );
        ensure!(
            matches!(escaped_identity(&fixture, &marker).await?, Some((ref after, false)) if after == &escaped),
            "settled cancellation left the exact escaped native child alive"
        );
        fixture.preserved(&sentinel_marker, &sentinel_before).await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    fixture.cleanup().await?;
    observed
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker, cached node:22-bookworm-slim and NATS; run with --ignored"]
async fn legacy_named_concurrency_lost_reply_and_exact_native_cancellation() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(true).await?;
    let observed = async {
        let marker = format!("family-{}", fresh_id());
        let first = fixture.request("shared", "creating-launch", &marker)?;
        let mut second = first.clone();
        second.launch_key = Some(LaunchKey::new("reused-launch")?);
        let (first, second) = tokio::join!(fixture.start(&first), fixture.start(&second));
        let first = first?;
        let second = second?;
        ensure!(
            first.execution_id == second.execution_id,
            "concurrent name created another execution"
        );
        let mut origins = [first.origin, second.origin];
        origins.sort_by_key(|origin| match origin {
            CommandLaunchOrigin::Created => 0,
            CommandLaunchOrigin::Reused => 1,
            _ => 2,
        });
        ensure!(
            origins == [CommandLaunchOrigin::Created, CommandLaunchOrigin::Reused],
            "atomic native provenance failed"
        );
        let before = fixture.ready(&marker).await.context("legacy_concurrent_initial_ready")?;
        let (created_key, reused_key) = if first.origin == CommandLaunchOrigin::Created {
            ("creating-launch", "reused-launch")
        } else {
            ("reused-launch", "creating-launch")
        };
        ensure!(
            matches!(fixture.cancel(reused_key).await?, CancelCommandLaunchResponse::Preserved { ref execution_id } if execution_id == &first.execution_id),
            "reused cancellation did not preserve work"
        );
        fixture.preserved(&marker, &before).await?;
        let target_marker = format!("lost-{}", fresh_id());
        let target = fixture.request("lost", "lost-started-reply", &target_marker)?;
        let subject = fixture.subject("run_command");
        let bytes = serde_json::to_vec(&target)?;
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(AUTH_HEADER, fixture.signer.sign_verb(&subject, &bytes, &fixture.scope)?);
        fixture
            .client
            .publish_with_reply_and_headers(subject, fixture.client.new_inbox(), headers, bytes.into())
            .await?;
        fixture.client.flush().await?;
        let target_before = fixture.ready(&target_marker).await.context("legacy_lost_target_ready")?;
        let replay = fixture.start(&target).await?;
        ensure!(
            replay.origin == CommandLaunchOrigin::Created,
            "lost Started replay changed origin"
        );
        fixture
            .preserved(&target_marker, &target_before)
            .await
            .context("legacy_lost_target_replay_preserved")?;
        let canceled = fixture.cancel("lost-started-reply").await?;
        ensure!(
            matches!(canceled, CancelCommandLaunchResponse::Canceled { ref execution_id } if execution_id.as_ref() == Some(&replay.execution_id)),
            "lost reply cancellation did not settle exact execution"
        );
        fixture.absent(&target_marker, &target_before).await?;
        ensure!(
            fixture.cancel("lost-started-reply").await? == canceled,
            "cancellation receipt is not replayable"
        );
        fixture.preserved(&marker, &before).await?;
        ensure!(
            matches!(fixture.cancel(created_key).await?, CancelCommandLaunchResponse::Canceled { .. }),
            "created cancellation did not settle"
        );
        fixture.absent(&marker, &before).await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    fixture.cleanup().await?;
    observed
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker, cached node:22-bookworm-slim and NATS; run with --ignored"]
async fn cancellation_before_start_and_unbound_admission_never_release_user_work() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(false).await?;
    let observed = async {
        let marker = format!("admission-{}", fresh_id());
        let request = fixture.request("admission", "admission-key", &marker)?;
        let refused: RunCommandResponse = fixture.send("run_command", &request, &fixture.scope).await?;
        ensure!(
            matches!(refused, RunCommandResponse::Denied(LeaseError::InvalidSpec { .. })),
            "unbound registration was treated as genuine Legacy"
        );
        ensure!(
            fixture.native(&marker, &[]).await?.members.is_empty(),
            "unbound request ran native work"
        );
        fixture.provider.bind_command_assignment(&fixture.payload).await?;
        let canceled = fixture.cancel("admission-key").await?;
        ensure!(
            canceled == CancelCommandLaunchResponse::Canceled { execution_id: None },
            "cancel-before-start has a fabricated execution"
        );
        let refused: RunCommandResponse = fixture.send("run_command", &request, &fixture.scope).await?;
        ensure!(
            matches!(refused, RunCommandResponse::Denied(_)),
            "canceled launch ran later"
        );
        let state = fixture.native(&marker, &[]).await?;
        ensure!(
            state.members.is_empty() && state.starts.is_empty(),
            "cancel-before-start released user work"
        );
        let positive_marker = format!("positive-{}", fresh_id());
        let positive = fixture.request("positive", "positive-key", &positive_marker)?;
        let (started, rebound) = tokio::join!(
            fixture.start(&positive),
            fixture.provider.bind_command_assignment(&fixture.payload)
        );
        let started = started?;
        rebound?;
        let before = fixture.ready(&positive_marker).await?;
        ensure!(
            fixture.start(&positive).await?.execution_id == started.execution_id,
            "same held re-admission orphaned valid custody"
        );
        fixture.preserved(&positive_marker, &before).await?;
        ensure!(
            matches!(
                fixture.cancel("positive-key").await?,
                CancelCommandLaunchResponse::Canceled { .. }
            ),
            "positive control failed"
        );
        fixture.absent(&positive_marker, &before).await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    fixture.cleanup().await?;
    observed
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker, cached node:22-bookworm-slim and NATS; run with --ignored"]
async fn foreign_scope_and_replaced_native_registration_cannot_reuse_custody() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(true).await?;
    let observed = async {
        let marker = format!("scope-{}", fresh_id());
        let request = fixture.request("scope", "scope-key", &marker)?;
        for kind in ["tenant", "worker", "attempt", "epoch", "scope_lease"] {
            let mut scope = fixture.scope.clone();
            match kind {
                "tenant" => scope.tenant = TenantId::new("foreign-tenant")?,
                "worker" => scope.worker_id = Some(WorkerId::new("foreign-worker")?),
                "attempt" => scope.attempt_id = Some(DispatchAttemptId::new("invented-attempt")?),
                "epoch" => scope.epoch = Some(LeaseEpoch::new(1)?),
                _ => scope.lease_id = LeaseId::new("foreign-lease".to_owned()),
            }
            let response: Value = fixture.send("run_command", &request, &scope).await?;
            ensure!(
                response["outcome"] == "denied" || response["error"] == "dispatch_auth",
                "foreign or malformed scope was accepted"
            );
        }
        let state = fixture.native(&marker, &[]).await?;
        ensure!(
            state.members.is_empty() && state.starts.is_empty(),
            "invalid scope reached user work"
        );
        let started = fixture.start(&request).await?;
        let before = fixture.ready(&marker).await?;
        let old_container = fixture.grant.sandbox_id.clone();
        fixture.provider.cancel(&fixture.lease).await?;
        let docker = bollard::Docker::connect_with_defaults()?;
        ensure!(
            matches!(
                docker.inspect_container(old_container.as_str(), None).await,
                Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
            ),
            "original container absence was not confirmed"
        );
        fixture.grant = fixture.provider.lease(&fixture.payload.request, &fixture.lease).await?;
        ensure!(
            fixture.grant.sandbox_id != old_container,
            "replacement reused canonical native identity"
        );
        fixture.provider.bind_command_assignment(&fixture.payload).await?;
        let response: RunCommandResponse = fixture.send("run_command", &request, &fixture.scope).await?;
        ensure!(
            matches!(response, RunCommandResponse::Denied(_)),
            "replacement replayed prior native launch"
        );
        ensure!(
            matches!(
                fixture.cancel("scope-key").await?,
                CancelCommandLaunchResponse::Pending { .. }
            ),
            "replacement fabricated prior cancellation settlement"
        );
        let state = fixture.native(&marker, &[]).await?;
        ensure!(
            state.members.is_empty() && state.starts.is_empty(),
            "replacement replayed user effects"
        );
        ensure!(
            !started.execution_id.as_str().is_empty() && before.records.len() == 2,
            "original positive controls missing"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    fixture.cleanup().await?;
    observed
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker, cached node:22-bookworm-slim and NATS; run with --ignored"]
async fn unsafe_and_full_custody_storage_refuses_before_native_bootstrap() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(true).await?;
    let observed = async {
        ensure!(
            matches!(
                fixture.cancel("storage-initializer").await?,
                CancelCommandLaunchResponse::Canceled { execution_id: None }
            ),
            "safe custody storage did not initialize"
        );
        let mut stream = fixture.js.get_stream(format!("KV_{CUSTODY_BUCKET}")).await?;
        let original = stream.info().await?.config.clone();
        for kind in ["expiry", "eviction", "transform", "republish", "full"] {
            let mut next = original.clone();
            match kind {
                "expiry" => next.max_age = Duration::from_secs(3600),
                "eviction" => next.discard = jetstream::stream::DiscardPolicy::Old,
                "transform" => {
                    next.subject_transform = Some(jetstream::stream::SubjectTransform {
                        source: format!("$KV.{CUSTODY_BUCKET}.>"),
                        destination: format!("$KV.{CUSTODY_BUCKET}.transformed.>"),
                    })
                }
                "republish" => {
                    next.republish = Some(jetstream::stream::Republish {
                        source: format!("$KV.{CUSTODY_BUCKET}.>"),
                        destination: "custody-refusal-copy.>".into(),
                        headers_only: false,
                    })
                }
                _ => next.max_messages = i64::try_from(stream.info().await?.state.messages)?,
            }
            fixture.js.update_stream(next.clone()).await?;
            let marker = format!("storage-{kind}-{}", fresh_id());
            let request = fixture.request(kind, &format!("storage-{kind}"), &marker)?;
            let response = fixture
                .send::<_, RunCommandResponse>("run_command", &request, &fixture.scope)
                .await;
            fixture.js.update_stream(original.clone()).await?;
            ensure!(
                stream.info().await?.config == original,
                "original live custody policy was not restored"
            );
            ensure!(
                matches!(response?, RunCommandResponse::Denied(_)),
                "unsafe or full storage released native work"
            );
            let state = fixture.native(&marker, &[]).await?;
            ensure!(
                state.members.is_empty() && state.starts.is_empty(),
                "storage refusal reached native user work"
            );
        }
        let marker = format!("storage-positive-{}", fresh_id());
        let request = fixture.request("restored", "restored-policy", &marker)?;
        fixture.start(&request).await?;
        let before = fixture.ready(&marker).await?;
        ensure!(
            matches!(
                fixture.cancel("restored-policy").await?,
                CancelCommandLaunchResponse::Canceled { .. }
            ),
            "restored policy positive failed"
        );
        fixture.absent(&marker, &before).await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    fixture.cleanup().await?;
    observed
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker, cached node:22-bookworm-slim and NATS; run with --ignored"]
async fn signed_ordinary_detached_elevated_command_preserves_execution_without_strong_custody() -> anyhow::Result<()> {
    let mut fixture = Fixture::with_capabilities(true, vec![ExtraCapability::new("SYS_PTRACE")?]).await?;
    let observed = async {
        let docker = bollard::Docker::connect_with_defaults()?;
        let container_id = fixture.grant.sandbox_id.as_str();
        let inspected = docker.inspect_container(container_id, None).await?;
        let host = inspected.host_config.as_ref();
        ensure!(
            inspected.id.as_deref() == Some(container_id)
                && host
                    .and_then(|host| host.cap_add.as_ref())
                    .is_some_and(|caps| caps.iter().any(|cap| cap == "SYS_PTRACE"))
                && host
                    .and_then(|host| host.cap_drop.as_ref())
                    .is_some_and(|caps| caps.iter().any(|cap| cap == "ALL")),
            "ordinary detached fixture lacks actual secure elevated configuration"
        );
        let root = SandboxUserName::new("root")?;
        let capabilities = fixture
            .provider
            .run_command_for_lease(
                &fixture.lease,
                vec!["node".into(), "-e".into(), CAPABILITY_PROBE.into()],
                Some("/tmp".into()),
                &CommandEnv::default(),
                5,
                Some(&root),
                StreamedOutput::Disabled,
                65536,
            )
            .await?;
        ensure!(
            capabilities.exit_code == 0,
            "ordinary detached actual capability probe failed"
        );
        let capabilities: Value = serde_json::from_slice(&capabilities.stdout)?;
        ensure!(
            capabilities["uid"] == 0 && capabilities["ptrace"] == true,
            "ordinary detached fixture did not establish actual root ptrace authority"
        );
        let denied_marker = format!("refused-strong-{}", fresh_id());
        let mut strong = fixture.request("refused-strong", "refused-strong", &denied_marker)?;
        strong.run_as_user = Some(root.clone());
        strong.cwd = Some("/tmp".into());
        let denied: RunCommandResponse = fixture.send("run_command", &strong, &fixture.scope).await?;
        ensure!(
            matches!(denied, RunCommandResponse::Denied(_)),
            "unsafe isolation admitted explicit strong custody"
        );
        let refused = fixture.native(&denied_marker, &[]).await?;
        ensure!(
            refused.records.is_empty() && refused.members.is_empty() && refused.starts.is_empty(),
            "explicit strong refusal released a user effect"
        );
        let marker = format!("ordinary-elevated-{}", fresh_id());
        let mut elevated = ordinary(fixture.request("ordinary-elevated", "unused-ordinary", &marker)?);
        elevated.run_as_user = Some(root);
        elevated.cwd = Some("/tmp".into());
        let started = fixture.start(&elevated).await?;
        let before = fixture.ready(&marker).await?;
        ensure!(
            started.origin == CommandLaunchOrigin::Created && before.starts == vec![marker.clone()],
            "signed ordinary elevated command was denied or duplicated"
        );
        let reused = fixture.start(&elevated).await?;
        ensure!(
            reused.origin == CommandLaunchOrigin::Reused && reused.execution_id == started.execution_id,
            "signed ordinary elevated command lost atomic named reuse"
        );
        fixture.preserved(&marker, &before).await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    fixture.cleanup().await?;
    observed
}

async fn write_unadmitted_control<W: tokio::io::AsyncWrite + Unpin>(
    input: &mut W,
    command: CommandWire,
) -> anyhow::Result<()> {
    let mut bytes = Vec::new();
    supervisor::write_command(&mut bytes, &command)?;
    input.write_all(&bytes).await?;
    input.flush().await?;
    Ok(())
}

async fn read_unadmitted_event<S>(output: &mut S, buffer: &mut Vec<u8>) -> anyhow::Result<EventWire>
where
    S: futures::Stream<Item = Result<bollard::container::LogOutput, bollard::errors::Error>> + Unpin,
{
    loop {
        if let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            let mut frame: Vec<u8> = buffer.drain(..=end).collect();
            frame.pop();
            return Ok(supervisor::decode_event(&frame)?.into_wire());
        }
        let chunk = output
            .next()
            .await
            .context("unadmitted helper stream ended before its control receipt")??;
        let message = match chunk {
            bollard::container::LogOutput::StdOut { message } => message,
            bollard::container::LogOutput::StdErr { message } if message.is_empty() => continue,
            _ => anyhow::bail!("unadmitted helper emitted unframed control data"),
        };
        ensure!(
            buffer.len() + message.len() <= supervisor::MAX_FRAME_BYTES,
            "unadmitted helper response exceeds its bound"
        );
        buffer.extend_from_slice(&message);
    }
}

async fn unadmitted_supervisor_observer_control(fixture: &Fixture) -> anyhow::Result<()> {
    eprintln!(
        "negative observer assets: owned_container_id={} owned_runtime_volume={}",
        fixture.grant.sandbox_id, fixture.runtime_volume
    );
    let candidates = fixture.observer_anchors().await?;
    ensure!(
        candidates.as_array().is_some_and(Vec::is_empty),
        "negative observer fixture already has admitted native custody"
    );
    let docker = bollard::Docker::connect_with_defaults()?;
    let container_id = fixture.grant.sandbox_id.as_str();
    let inspected = docker.inspect_container(container_id, None).await?;
    let mounts: Vec<_> = inspected
        .mounts
        .as_ref()
        .context("negative observer mounts missing")?
        .iter()
        .filter(|mount| mount.name.as_deref() == Some(fixture.runtime_volume.as_str()))
        .collect();
    ensure!(
        inspected.id.as_deref() == Some(container_id)
            && mounts.len() == 1
            && mounts[0].typ == Some(bollard::config::MountPointTypeEnum::VOLUME)
            && mounts[0].rw == Some(false),
        "negative observer immutable helper provenance changed"
    );
    let executable = format!(
        "{}/supervisor",
        mounts[0]
            .destination
            .as_deref()
            .context("negative observer runtime destination missing")?
    );
    let id = supervisor::ExecutionId::new(fresh_id())?;
    let exec = docker
        .create_exec(
            container_id,
            bollard::exec::CreateExecOptions {
                cmd: Some(vec![executable, "--execution-id".into(), id.as_str().into()]),
                working_dir: Some("/tmp".into()),
                attach_stdin: Some(true),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await?;
    let started = docker
        .start_exec(&exec.id, None::<bollard::exec::StartExecOptions>)
        .await?;
    let bollard::exec::StartExecResults::Attached { mut input, mut output } = started else {
        anyhow::bail!("negative observer helper transport is detached");
    };
    let mut buffer = Vec::new();
    let mut native = None;
    let observed = tokio::time::timeout(Duration::from_secs(20), async {
        let EventWire::Hello {
            version,
            execution_id,
            mut environment,
        } = read_unadmitted_event(&mut output, &mut buffer).await?
        else {
            anyhow::bail!("negative observer helper did not establish its private greeting");
        };
        ensure!(
            version == supervisor::VERSION && execution_id == id,
            "negative observer helper greeting changed"
        );
        environment.insert(CommandEnv::RESERVED_KEY.into(), id.as_str().into());
        write_unadmitted_control(
            &mut input,
            CommandWire::Init {
                version: supervisor::VERSION,
                execution_id: id.clone(),
                argv: vec![
                    "node".into(),
                    "-e".into(),
                    "process.stdout.write('unadmitted-ready');setInterval(()=>{},1000)".into(),
                ],
                env: environment,
                cwd: Some("/tmp".into()),
                timeout_ms: Some(30000),
                release_timeout_ms: Some(30000),
            },
        )
        .await?;
        let EventWire::Ready {
            version,
            execution_id,
            helper,
            root,
            ..
        } = read_unadmitted_event(&mut output, &mut buffer).await?
        else {
            anyhow::bail!("negative observer helper never established native readiness");
        };
        ensure!(
            version == supervisor::VERSION
                && execution_id == id
                && helper.pid > 1
                && root.pid > 1
                && helper.pid != root.pid
                && helper.start_time > 0
                && root.start_time > 0,
            "negative observer helper identity is invalid"
        );
        native = Some((helper, root));
        eprintln!(
            "negative observer native pins: helper_pid={} helper_start={} root_pid={} root_start={}",
            helper.pid, helper.start_time, root.pid, root.start_time
        );
        write_unadmitted_control(&mut input, CommandWire::Release { execution_id: id.clone() }).await?;
        ensure!(
            matches!(read_unadmitted_event(&mut output, &mut buffer).await?, EventWire::Released { execution_id } if execution_id == id),
            "negative observer helper never confirmed release"
        );
        let mut ready = Vec::new();
        while ready != b"unadmitted-ready" {
            let EventWire::Output {
                execution_id,
                stream: supervisor::Stream::Stdout,
                data,
            } = read_unadmitted_event(&mut output, &mut buffer).await?
            else {
                anyhow::bail!("negative observer payload never executed its bounded readiness effect");
            };
            ensure!(
                execution_id == id && ready.len() + data.len() <= 16,
                "negative observer readiness output changed"
            );
            ready.extend(data);
            ensure!(
                b"unadmitted-ready".starts_with(&ready),
                "negative observer readiness output changed"
            );
        }
        ensure!(
            fixture.observer_anchors().await?.as_array().is_some_and(Vec::is_empty),
            "raw helper unexpectedly gained admitted custody"
        );
        let marker = format!("unadmitted-{}", fresh_id());
        let failure = fixture
            .native(&marker, &[])
            .await
            .err()
            .context("observer silently accepted an unknown nondumpable helper")?;
        let denied = failure
            .downcast_ref::<CensusPermissionDiagnostic>()
            .context("unknown helper refusal was not the actual census permission boundary")?;
        ensure!(
            denied.pid == helper.pid
                && denied.start.as_deref() == Some(helper.start_time.to_string().as_str())
                && matches!(denied.anchor_class, DeniedAnchorClass::Unknown)
                && denied.retained.is_empty()
                && denied.candidates.is_empty()
                && denied.verified.is_empty(),
            "observer did not fail closed on the exact unadmitted helper identity"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await;
    let helper_cleanup = tokio::time::timeout(Duration::from_secs(10), async {
        write_unadmitted_control(
            &mut input,
            CommandWire::Signal {
                execution_id: id.clone(),
                request_id: 1,
                signal: 9,
            },
        )
        .await?;
        let mut signaled = false;
        loop {
            match read_unadmitted_event(&mut output, &mut buffer).await? {
                EventWire::SignalResult {
                    execution_id,
                    request_id: 1,
                    signal: 9,
                    delivered: Delivery::Delivered,
                } if execution_id == id => signaled = true,
                EventWire::RootExited { execution_id, .. } if execution_id == id => {}
                EventWire::FamilyAbsent {
                    execution_id,
                    released: ReleaseState::Released,
                    canceled: Cancellation::Canceled,
                    reaped_count,
                    ..
                } if execution_id == id && reaped_count > 0 => {
                    ensure!(
                        signaled,
                        "negative observer helper cleanup lacks actual signal acknowledgement"
                    );
                    break;
                }
                _ => anyhow::bail!("negative observer helper cleanup remains unconfirmed"),
            }
        }
        write_unadmitted_control(&mut input, CommandWire::Acknowledge { execution_id: id.clone() }).await?;
        input.shutdown().await?;
        while let Some(chunk) = output.next().await {
            let chunk = chunk?;
            ensure!(
                matches!(chunk, bollard::container::LogOutput::StdOut { ref message } if message.is_empty()),
                "negative observer helper emitted data after native absence"
            );
        }
        ensure!(buffer.is_empty(), "negative observer helper left an incomplete terminal frame");
        let inspection = docker.inspect_exec(&exec.id).await?;
        ensure!(
            inspection.running == Some(false),
            "negative observer helper did not terminate"
        );
        let (helper, root) = native.context("negative observer cleanup has no independently pinned family")?;
        let identities = json!([
            { "pid": helper.pid, "start": helper.start_time.to_string() },
            { "pid": root.pid, "start": root.start_time.to_string() },
        ]);
        let output = fixture
            .provider
            .run_command_for_lease(
                &fixture.lease,
                vec!["node".into(), "-e".into(), ALIVE_PROBE.into(), identities.to_string()],
                Some("/tmp".into()),
                &CommandEnv::default(),
                5,
                None,
                StreamedOutput::Disabled,
                65536,
            )
            .await?;
        ensure!(
            output.exit_code == 0 && serde_json::from_slice::<Vec<bool>>(&output.stdout)? == vec![false, false],
            "negative observer exact native family survived teardown"
        );
        let after = fixture.native("unadmitted-after-cleanup", &[]).await?;
        ensure!(
            after.records.is_empty() && after.members.is_empty() && after.starts.is_empty() && after.anchors.is_empty(),
            "observer did not return to a clean exact native census after unknown helper teardown"
        );
        eprintln!(
            "negative observer native cleanup: exact_helper_absent=true exact_root_absent=true private_control_closed=true fallback_used=false"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await;
    if let Ok(Err(error)) = tokio::time::timeout(Duration::from_secs(1), input.shutdown()).await {
        eprintln!("negative observer helper input was already closed: {error}");
    }
    helper_cleanup.context("negative observer helper cleanup exceeded its bound")??;
    observed.context("negative observer control exceeded its setup/observation bound")?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMMAND_SUPERVISOR, Docker, cached node:22-bookworm-slim and NATS; run with --ignored"]
async fn unadmitted_immutable_nondumpable_helper_is_never_an_observer_exemption() -> anyhow::Result<()> {
    let mut fixture = Fixture::new(true).await?;
    let observed = unadmitted_supervisor_observer_control(&fixture).await;
    fixture.cleanup().await?;
    observed
}
