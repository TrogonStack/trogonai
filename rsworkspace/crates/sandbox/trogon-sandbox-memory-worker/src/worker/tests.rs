use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CommandOutput, ExecutionId, FileOpErrorKind, FileOpResponse, FilePresence, FileResultTruncation,
    FileWatchEventKind, LeaseId, ListProcessesResponse, NativeHandleResponse, PollCommandResponse, ProcessName,
    ProcessState, ProviderKind, ReadinessNeedle, ReadinessProbe, RestartCommandResponse, RunCommandRequest,
    RunCommandResponse, StopCommandResponse, StopSignal, UnixSignal, WaitReadyRequest, WaitReadyResponse, WorkerId,
};
use trogon_sandbox_protocol::{AuthorityHost, FreezeOutcome, HeldLease, PurgeOutcome, ThawOutcome};

use super::authority_host::MemoryAuthorityHost;
use super::command_launch::{run_command, signal_number};
use super::command_task::ClosePolicy;
use super::exec_state::ExecState;
use super::handle_verb::handle_verb;
use super::lease::Lease;
use super::lease_task::run_lease_task;
use super::proc_entry::Proc;
use super::process_control::{restart_command, stop_command};
use super::process_query::{list_processes, wait_ready};
use super::route::check_native;
use super::state::State;
use super::verb::{ServedVerb, UnsupportedVerb, Verb};
use crate::mutex_ext::MutexExt;
use crate::{CommandTable, FrameSink, LiveOutput};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Discard;

#[async_trait]
impl FrameSink for Discard {
    async fn publish(&self, _subject: String, _payload: Bytes) {}
}

fn state() -> Result<Arc<State>, Box<dyn std::error::Error>> {
    Ok(Arc::new(State::new(
        ProviderKind::Mock,
        CommandTable::default(),
        WorkerId::new("w1")?,
    )))
}

async fn held(state: &State, id: &str) -> Result<(LeaseId, Arc<Lease>), Box<dyn std::error::Error>> {
    let id = LeaseId::new(id);
    state.grant(&id, None).await.map_err(|error| error.to_string())?;
    let lease = state.lease(&id).ok_or("lease was not granted")?;
    Ok((id, lease))
}

fn request(json: serde_json::Value) -> Result<RunCommandRequest, serde_json::Error> {
    serde_json::from_value(json)
}

fn detached(command: &[&str]) -> Result<RunCommandRequest, serde_json::Error> {
    request(serde_json::json!({"lease_id": "l1", "command": command, "detached": true}))
}

async fn offline_client() -> Result<async_nats::Client, Box<dyn std::error::Error>> {
    Ok(async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .connect("nats://127.0.0.1:1")
        .await?)
}

#[tokio::test]
async fn a_closing_lease_refuses_new_work() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    lease.closing.store(true, Ordering::SeqCst);
    let reply = run_command(&state, &lease, detached(&["true"])?).await;
    assert!(matches!(reply, RunCommandResponse::Denied(_)));
    assert!(run_lease_task(&lease, ClosePolicy::Abort, async { 1 }).await.is_none());
    Ok(())
}

#[tokio::test]
async fn a_shutting_down_worker_grants_nothing() -> TestResult {
    let state = state()?;
    state.shutting_down.store(true, Ordering::SeqCst);
    assert!(state.grant(&LeaseId::new("l1"), None).await.is_err());
    Ok(())
}

#[tokio::test]
async fn stop_needs_a_known_execution_with_a_task() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    let orphan = ExecutionId::new("orphan");
    lease
        .execs
        .lock_clean()
        .insert(orphan.clone(), ExecState::Done(CommandOutput::default()));
    assert!(matches!(
        stop_command(&lease, &orphan, Duration::ZERO).await,
        StopCommandResponse::NotFound
    ));
    Ok(())
}

#[tokio::test]
async fn stop_escalates_to_kill_when_the_command_ignores_its_signal() -> TestResult {
    struct Stubborn;
    #[async_trait]
    impl crate::CommandHandler for Stubborn {
        async fn run(&self, _ctx: crate::CommandCtx) -> CommandOutput {
            std::future::pending().await
        }
    }
    let mut commands = CommandTable::empty();
    commands.register("stubborn", Stubborn);
    let state = Arc::new(State::new(ProviderKind::Mock, commands, WorkerId::new("w1")?));
    let (_, lease) = held(&state, "l1").await?;
    let RunCommandResponse::Started(started) = run_command(&state, &lease, detached(&["stubborn"])?).await else {
        return Err("not started".into());
    };
    let reply = stop_command(&lease, &started.execution_id, Duration::ZERO).await;
    assert!(matches!(
        reply,
        StopCommandResponse::Stopped {
            signal: StopSignal::Kill
        }
    ));
    Ok(())
}

#[tokio::test]
async fn restart_reports_what_the_relaunch_answers() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;

    let attached = ExecutionId::new("attached");
    lease.launches.lock_clean().insert(
        attached.clone(),
        request(serde_json::json!({"lease_id": "l1", "command": ["true"]}))?,
    );
    assert!(matches!(
        restart_command(&state, &lease, &attached, Duration::ZERO).await,
        RestartCommandResponse::Denied(_)
    ));

    let unsupported = ExecutionId::new("unsupported");
    lease.launches.lock_clean().insert(
        unsupported.clone(),
        request(serde_json::json!({"lease_id": "l1", "command": ["true"], "detached": true, "stdin": "interactive"}))?,
    );
    assert!(matches!(
        restart_command(&state, &lease, &unsupported, Duration::ZERO).await,
        RestartCommandResponse::Denied(_)
    ));
    Ok(())
}

#[tokio::test]
async fn wait_ready_needs_an_execution_behind_the_process() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    let id = ExecutionId::new("ghost");
    lease.procs.lock_clean().insert(
        id.clone(),
        Proc {
            name: Some(ProcessName::new("ghost")?),
            readiness: Some(ReadinessProbe::OutputContains {
                needle: ReadinessNeedle::new("up")?,
            }),
            live: LiveOutput::default(),
        },
    );
    let reply = wait_ready(
        &lease,
        WaitReadyRequest {
            lease_id: LeaseId::new("l1"),
            execution_id: id,
            timeout_seconds: 1,
        },
    )
    .await;
    assert!(matches!(reply, WaitReadyResponse::NotFound));
    Ok(())
}

#[tokio::test]
async fn the_authority_host_acts_only_on_the_leases_it_holds() -> TestResult {
    let state = state()?;
    let (id, _) = held(&state, "l1").await?;
    let host = MemoryAuthorityHost {
        state: Arc::clone(&state),
        client: offline_client().await?,
    };
    let leases = host.held_leases().await?;
    let [lease] = leases.as_slice() else {
        return Err("expected one held lease".into());
    };
    assert_eq!(host.freeze(lease).await?, FreezeOutcome::Unsupported);
    assert_eq!(host.thaw(lease).await?, ThawOutcome::NoLongerOwned);

    let HeldLease::Provider { resource_id, .. } = lease else {
        return Err("expected a provider lease".into());
    };
    let foreign = HeldLease::Provider {
        lease_id: id.clone(),
        provider: ProviderKind::Daytona,
        resource_id: resource_id.clone(),
    };
    assert_eq!(host.freeze(&foreign).await?, FreezeOutcome::Stale);
    assert_eq!(host.thaw(&foreign).await?, ThawOutcome::Stale);
    assert_eq!(host.purge(&foreign).await?, PurgeOutcome::Stale);

    assert_eq!(host.purge(lease).await?, PurgeOutcome::Purged);
    assert_eq!(host.purge(lease).await?, PurgeOutcome::Settled);
    assert_eq!(host.freeze(lease).await?, FreezeOutcome::Stale);
    let unknown = HeldLease::Provider {
        lease_id: LeaseId::new("unknown"),
        provider: ProviderKind::Mock,
        resource_id: resource_id.clone(),
    };
    assert_eq!(host.purge(&unknown).await?, PurgeOutcome::Stale);
    assert!(state.lease(&id).is_none());
    Ok(())
}

#[tokio::test]
async fn closing_a_lease_twice_is_harmless() -> TestResult {
    let state = state()?;
    let (id, _) = held(&state, "l1").await?;
    state.close_lease(&Discard, &id).await;
    state.close_lease(&Discard, &id).await;
    assert!(state.held_leases().is_empty());
    Ok(())
}

#[test]
fn every_signal_maps_to_its_number() {
    let numbers: Vec<i32> = [
        UnixSignal::Hup,
        UnixSignal::Int,
        UnixSignal::Quit,
        UnixSignal::Kill,
        UnixSignal::Usr1,
        UnixSignal::Usr2,
        UnixSignal::Term,
    ]
    .into_iter()
    .map(signal_number)
    .collect();
    assert_eq!(numbers, [1, 2, 3, 9, 10, 12, 15]);
}

struct Ignores;

#[async_trait]
impl crate::CommandHandler for Ignores {
    async fn run(&self, _ctx: crate::CommandCtx) -> CommandOutput {
        // Ignores its cancellation token, like a process ignoring SIGTERM.
        std::future::pending().await
    }
}

fn named(command: &[&str], process: Value) -> Result<RunCommandRequest, serde_json::Error> {
    request(json!({"lease_id": "l1", "command": command, "detached": true, "process": process}))
}

fn started(reply: RunCommandResponse) -> Result<ExecutionId, Box<dyn std::error::Error>> {
    match reply {
        RunCommandResponse::Started(started) => Ok(started.execution_id),
        other => Err(format!("expected Started, got {other:?}").into()),
    }
}

fn wait(execution_id: &ExecutionId, timeout_seconds: u32) -> WaitReadyRequest {
    WaitReadyRequest {
        lease_id: LeaseId::new("l1"),
        execution_id: execution_id.clone(),
        timeout_seconds,
    }
}

async fn verb(
    state: &State,
    lease: &Arc<Lease>,
    verb: ServedVerb,
    body: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let reply = handle_verb(state, lease, verb, &serde_json::to_vec(&body)?).await?;
    Ok(serde_json::from_slice(&reply)?)
}

async fn file_op(state: &State, lease: &Arc<Lease>, op: Value) -> Result<FileOpResponse, Box<dyn std::error::Error>> {
    let reply = handle_verb(
        state,
        lease,
        ServedVerb::FileOp,
        &serde_json::to_vec(&json!({"lease_id": "l1", "op": op}))?,
    )
    .await?;
    Ok(serde_json::from_slice(&reply)?)
}

fn put(lease: &Lease, path: &str, body: &str) {
    lease.fs.lock_clean().write(path, body.as_bytes().to_vec());
}

#[test]
fn every_unsupported_verb_has_a_typed_reply() -> TestResult {
    for token in [
        "checkpoint_running",
        "network_policy_update",
        "write_stdin",
        "create_user",
        "create_group",
        "modify_group_membership",
        "get_domain",
    ] {
        let Some(Verb::Unsupported(unsupported)) = Verb::parse(token) else {
            return Err(format!("{token} is not an unsupported verb").into());
        };
        assert!(!UnsupportedVerb::reply(unsupported)?.is_empty(), "{token}");
    }
    assert_eq!(Verb::parse("terminal"), Some(Verb::Terminal));
    assert_eq!(Verb::parse("file_op"), Some(Verb::Served(ServedVerb::FileOp)));
    assert!(Verb::parse("lease").is_none());
    Ok(())
}

#[test]
fn pause_resume_suspend_and_wake_are_served_verbs() {
    assert_eq!(Verb::parse("pause"), Some(Verb::Served(ServedVerb::Pause)));
    assert_eq!(Verb::parse("resume"), Some(Verb::Served(ServedVerb::Resume)));
    assert_eq!(Verb::parse("suspend"), Some(Verb::Served(ServedVerb::Suspend)));
    assert_eq!(Verb::parse("wake"), Some(Verb::Served(ServedVerb::Wake)));
}

#[test]
fn native_options_are_checked_against_the_fake_allowlist() {
    let ok = json!({"mock": {"label": "x"}});
    assert!(check_native(ProviderKind::Mock, Some(&ok)).is_ok());
    assert!(check_native(ProviderKind::Mock, None).is_ok());
    let unknown = json!({"mock": {"bogus": 1}});
    assert!(check_native(ProviderKind::Mock, Some(&unknown)).is_err());
}

#[tokio::test]
async fn the_native_handle_reports_the_lease_and_its_region() -> TestResult {
    let state = state()?;
    let id = LeaseId::new("l1");
    state
        .grant(&id, Some("eu".to_string()))
        .await
        .map_err(|error| error.to_string())?;
    let lease = state.lease(&id).ok_or("lease was not granted")?;
    let reply = handle_verb(&state, &lease, ServedVerb::NativeHandle, b"{}").await?;
    let NativeHandleResponse::Available(handle) = serde_json::from_slice(&reply)? else {
        return Err("expected an available handle".into());
    };
    assert_eq!(handle.vendor_id, "memory-l1");
    assert_eq!(handle.region.as_deref(), Some("eu"));
    Ok(())
}

#[tokio::test]
async fn stop_ends_a_cooperative_command_with_sigterm() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    let id = started(run_command(&state, &lease, named(&["serve"], json!({"name": "api"}))?).await)?;
    assert_eq!(
        stop_command(&lease, &id, Duration::from_secs(5)).await,
        StopCommandResponse::Stopped {
            signal: StopSignal::Term
        }
    );
    assert_eq!(lease.exit_code(&id), Some(Some(143)));
    assert_eq!(
        stop_command(&lease, &id, Duration::from_secs(5)).await,
        StopCommandResponse::AlreadyCompleted
    );
    assert_eq!(
        stop_command(&lease, &ExecutionId::new("missing"), Duration::ZERO).await,
        StopCommandResponse::NotFound
    );
    Ok(())
}

#[tokio::test]
async fn stop_escalates_to_sigkill_after_the_grace_period() -> TestResult {
    let mut commands = CommandTable::empty();
    commands.register("ignores", Ignores);
    let state = Arc::new(State::new(ProviderKind::Mock, commands, WorkerId::new("w1")?));
    let (_, lease) = held(&state, "l1").await?;
    let id = started(run_command(&state, &lease, detached(&["ignores"])?).await)?;
    assert_eq!(
        stop_command(&lease, &id, Duration::from_millis(50)).await,
        StopCommandResponse::Stopped {
            signal: StopSignal::Kill
        }
    );
    assert_eq!(lease.exit_code(&id), Some(Some(137)));
    Ok(())
}

#[tokio::test]
async fn restart_reruns_the_original_request_under_a_new_execution_id() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    let probe = json!({"name": "api", "readiness": {"kind": "tcp_port", "port": 8080}});
    let first = started(run_command(&state, &lease, named(&["serve", "8080"], probe)?).await)?;
    assert_eq!(wait_ready(&lease, wait(&first, 5)).await, WaitReadyResponse::Ready);
    let RestartCommandResponse::Restarted { execution_id: second } =
        restart_command(&state, &lease, &first, Duration::from_secs(5)).await
    else {
        return Err("expected Restarted".into());
    };
    assert_ne!(first, second);
    // The probe runs again against the new execution.
    assert_eq!(wait_ready(&lease, wait(&second, 5)).await, WaitReadyResponse::Ready);
    let ListProcessesResponse::Processes { processes } = list_processes(&lease) else {
        return Err("expected processes".into());
    };
    let running: Vec<_> = processes
        .iter()
        .filter(|process| process.state == ProcessState::Running)
        .collect();
    assert_eq!(running.len(), 1);
    assert_eq!(running.first().map(|process| &process.execution_id), Some(&second));
    assert_eq!(
        running
            .first()
            .and_then(|process| process.name.as_ref())
            .map(ProcessName::as_str),
        Some("api")
    );
    assert_eq!(
        restart_command(&state, &lease, &ExecutionId::new("missing"), Duration::ZERO).await,
        RestartCommandResponse::NotFound
    );
    Ok(())
}

#[tokio::test]
async fn a_same_name_start_returns_the_running_execution() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    let launch = named(&["serve"], json!({"name": "api"}))?;
    let first = started(run_command(&state, &lease, launch.clone()).await)?;
    let second = started(run_command(&state, &lease, launch).await)?;
    assert_eq!(first, second);
    let ListProcessesResponse::Processes { processes } = list_processes(&lease) else {
        return Err("expected processes".into());
    };
    assert_eq!(processes.len(), 1);
    assert_eq!(
        processes
            .first()
            .and_then(|process| process.name.as_ref())
            .map(ProcessName::as_str),
        Some("api")
    );
    assert_eq!(
        processes.first().map(|process| &process.state),
        Some(&ProcessState::Running)
    );
    Ok(())
}

#[tokio::test]
async fn a_name_is_free_again_once_its_process_exited() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    let launch = named(&["true"], json!({"name": "job"}))?;
    let first = started(run_command(&state, &lease, launch.clone()).await)?;
    while lease.exit_code(&first) == Some(None) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let second = started(run_command(&state, &lease, launch).await)?;
    assert_ne!(first, second);
    Ok(())
}

#[tokio::test]
async fn readiness_reports_ready_not_ready_and_failed() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    let probe =
        |name: &str, needle: &str| json!({"name": name, "readiness": {"kind": "output_contains", "needle": needle}});

    let up = started(
        run_command(
            &state,
            &lease,
            named(&["serve", "8080"], probe("up", "listening on 8080"))?,
        )
        .await,
    )?;
    assert_eq!(wait_ready(&lease, wait(&up, 5)).await, WaitReadyResponse::Ready);

    let quiet = started(run_command(&state, &lease, named(&["serve"], probe("quiet", "never"))?).await)?;
    assert_eq!(wait_ready(&lease, wait(&quiet, 0)).await, WaitReadyResponse::NotReady);

    let dead = started(run_command(&state, &lease, named(&["false"], probe("dead", "never"))?).await)?;
    assert_eq!(
        wait_ready(&lease, wait(&dead, 5)).await,
        WaitReadyResponse::Failed { exit_code: 1 }
    );

    assert_eq!(
        wait_ready(&lease, wait(&ExecutionId::new("missing"), 0)).await,
        WaitReadyResponse::NotFound
    );
    Ok(())
}

#[tokio::test]
async fn a_tcp_probe_follows_the_listening_port() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    let tcp = |name: &str, port: u16| json!({"name": name, "readiness": {"kind": "tcp_port", "port": port}});
    let listening = started(run_command(&state, &lease, named(&["serve", "9000"], tcp("tcp", 9000))?).await)?;
    assert_eq!(wait_ready(&lease, wait(&listening, 5)).await, WaitReadyResponse::Ready);
    let elsewhere = started(run_command(&state, &lease, named(&["serve", "9001"], tcp("tcp2", 9002))?).await)?;
    assert_eq!(
        wait_ready(&lease, wait(&elsewhere, 0)).await,
        WaitReadyResponse::NotReady
    );
    Ok(())
}

#[tokio::test]
async fn a_process_spec_needs_a_detached_command_and_a_valid_name() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    for body in [
        json!({"lease_id": "l1", "command": ["true"], "detached": false, "process": {"name": "ok"}}),
        json!({"lease_id": "l1", "command": ["true"], "detached": true, "process": {"name": "has space"}}),
    ] {
        let reply = verb(&state, &lease, ServedVerb::RunCommand, body).await?;
        assert_eq!(reply["outcome"], "denied", "{reply}");
    }
    assert!(lease.execs.lock_clean().is_empty());
    Ok(())
}

#[tokio::test]
async fn memory_suspend_freezes_the_lease_and_wake_restores_it() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    let level = |level: &str| json!({"lease_id": "l1", "generation": 1, "level": level});
    // A disk level is not offered, and leaves the lease running.
    let reply = verb(&state, &lease, ServedVerb::Suspend, level("disk")).await?;
    assert_eq!(reply["outcome"], "unsupported");
    assert!(!lease.paused.load(Ordering::SeqCst));
    // A repeat of the same generation is success.
    for _ in 0..2 {
        let reply = verb(&state, &lease, ServedVerb::Suspend, level("memory")).await?;
        assert_eq!(reply["outcome"], "done");
    }
    let reply = verb(&state, &lease, ServedVerb::RunCommand, json!({})).await?;
    assert_eq!(reply["outcome"], "denied");
    let reply = verb(&state, &lease, ServedVerb::Wake, level("disk")).await?;
    assert_eq!(reply["outcome"], "unsupported");
    assert!(lease.paused.load(Ordering::SeqCst));
    for _ in 0..2 {
        let reply = verb(&state, &lease, ServedVerb::Wake, level("memory")).await?;
        assert_eq!(reply["outcome"], "done");
    }
    assert!(!lease.paused.load(Ordering::SeqCst));
    Ok(())
}

#[tokio::test]
async fn pause_blocks_commands_and_resume_restores_them_idempotently() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    for _ in 0..2 {
        let reply = verb(&state, &lease, ServedVerb::Pause, json!({})).await?;
        assert_eq!(reply["outcome"], "done");
    }
    // The paused guard answers before the body is parsed.
    let reply = verb(&state, &lease, ServedVerb::RunCommand, json!({})).await?;
    assert_eq!(reply["outcome"], "denied");
    assert!(reply.to_string().contains("lease is paused"), "{reply}");
    for _ in 0..2 {
        let reply = verb(&state, &lease, ServedVerb::Resume, json!({})).await?;
        assert_eq!(reply["outcome"], "done");
    }
    assert!(!lease.paused.load(Ordering::SeqCst));
    let reply = verb(
        &state,
        &lease,
        ServedVerb::RunCommand,
        json!({"lease_id": "l1", "command": ["true"]}),
    )
    .await?;
    assert_eq!(reply["outcome"], "completed", "{reply}");
    Ok(())
}

fn echo(detached: bool, secret: &str) -> Value {
    json!({
        "lease_id": "l1",
        "command": ["echo", "token", secret],
        "detached": detached,
        "redact_values": [secret],
    })
}

#[tokio::test]
async fn a_registered_value_is_redacted_from_command_output() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    let reply = verb(
        &state,
        &lease,
        ServedVerb::RunCommand,
        echo(false, "s3cr3t-token-value"),
    )
    .await?;
    let RunCommandResponse::Completed(output) = serde_json::from_value(reply)? else {
        return Err("expected completed".into());
    };
    assert_eq!(output.stdout, b"token [REDACTED]\n");
    Ok(())
}

#[tokio::test]
async fn a_registered_value_is_redacted_from_a_polled_detached_result() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    let reply = verb(&state, &lease, ServedVerb::RunCommand, echo(true, "s3cr3t-token-value")).await?;
    let RunCommandResponse::Started(started) = serde_json::from_value(reply)? else {
        return Err("expected started".into());
    };
    let poll = json!({"lease_id": "l1", "execution_id": started.execution_id});
    for _ in 0..100 {
        let reply = verb(&state, &lease, ServedVerb::PollCommand, poll.clone()).await?;
        if let PollCommandResponse::Completed(output) = serde_json::from_value(reply)? {
            assert_eq!(output.stdout, b"token [REDACTED]\n");
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("the detached command never completed".into())
}

#[tokio::test]
async fn output_is_untouched_when_no_value_is_registered() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    let body = json!({"lease_id": "l1", "command": ["echo", "s3cr3t-token-value"]});
    let RunCommandResponse::Completed(output) =
        serde_json::from_value(verb(&state, &lease, ServedVerb::RunCommand, body).await?)?
    else {
        return Err("expected completed".into());
    };
    assert_eq!(output.stdout, b"s3cr3t-token-value\n");
    Ok(())
}

#[tokio::test]
async fn changes_since_lease_start_and_since_a_checkpoint_are_exact() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    put(&lease, "/w/a", "1");
    put(&lease, "/w/b", "1");
    assert_eq!(
        file_op(&state, &lease, json!({"op": "changes"})).await?,
        FileOpResponse::Changes {
            since: FileOpResponse::CHANGES_SINCE_START.to_string(),
            added: vec!["/w/a".to_string(), "/w/b".to_string()],
            modified: vec![],
            deleted: vec![],
            truncated: FileResultTruncation::Complete,
        }
    );

    let FileOpResponse::Checkpointed { checkpoint_id, entries } =
        file_op(&state, &lease, json!({"op": "checkpoint", "path": "/w"})).await?
    else {
        return Err("expected a checkpoint".into());
    };
    assert_eq!(entries, 2);
    put(&lease, "/w/a", "2");
    put(&lease, "/w/c", "1");
    // Rewriting identical bytes is not a modification.
    put(&lease, "/w/b", "1");
    assert_eq!(
        file_op(&state, &lease, json!({"op": "changes", "since": checkpoint_id})).await?,
        FileOpResponse::Changes {
            since: checkpoint_id,
            added: vec!["/w/c".to_string()],
            modified: vec!["/w/a".to_string()],
            deleted: vec![],
            truncated: FileResultTruncation::Complete,
        }
    );
    Ok(())
}

#[tokio::test]
async fn a_watch_streams_the_in_memory_files_and_stops() -> TestResult {
    let state = state()?;
    let (_, lease) = held(&state, "l1").await?;
    let FileOpResponse::WatchStarted { watch_id } = file_op(
        &state,
        &lease,
        json!({"op": "watch_start", "path": "/w", "recursive": true}),
    )
    .await?
    else {
        return Err("expected a watch".into());
    };
    put(&lease, "/w/x/new", "1");
    let FileOpResponse::WatchEvents { events, next_seq, .. } = file_op(
        &state,
        &lease,
        json!({"op": "watch_poll", "watch_id": watch_id, "wait_ms": 500}),
    )
    .await?
    else {
        return Err("expected events".into());
    };
    assert_eq!(
        events
            .iter()
            .map(|event| (event.kind, event.path.as_str()))
            .collect::<Vec<_>>(),
        vec![(FileWatchEventKind::Created, "/w/x/new")]
    );
    put(&lease, "/w/x/new", "22");
    let FileOpResponse::WatchEvents { events, .. } = file_op(
        &state,
        &lease,
        json!({"op": "watch_poll", "watch_id": watch_id, "after_seq": next_seq, "wait_ms": 500}),
    )
    .await?
    else {
        return Err("expected events".into());
    };
    assert_eq!(
        events.first().map(|event| event.kind),
        Some(FileWatchEventKind::Modified)
    );
    assert_eq!(
        file_op(&state, &lease, json!({"op": "watch_stop", "watch_id": watch_id})).await?,
        FileOpResponse::WatchStopped {
            existed: FilePresence::Present
        }
    );
    Ok(())
}

#[tokio::test]
async fn another_lease_cannot_use_a_checkpoint_or_watch_and_other_file_ops_are_unsupported() -> TestResult {
    let state = state()?;
    let (_, a) = held(&state, "a").await?;
    let (_, b) = held(&state, "b").await?;
    let FileOpResponse::Checkpointed { checkpoint_id, .. } =
        file_op(&state, &a, json!({"op": "checkpoint", "path": "/"})).await?
    else {
        return Err("expected a checkpoint".into());
    };
    let FileOpResponse::WatchStarted { watch_id } =
        file_op(&state, &a, json!({"op": "watch_start", "path": "/"})).await?
    else {
        return Err("expected a watch".into());
    };
    assert!(matches!(
        file_op(&state, &b, json!({"op": "changes", "since": checkpoint_id})).await?,
        FileOpResponse::Failed {
            kind: FileOpErrorKind::UnknownCheckpoint,
            ..
        }
    ));
    assert!(matches!(
        file_op(&state, &b, json!({"op": "watch_poll", "watch_id": watch_id})).await?,
        FileOpResponse::Failed {
            kind: FileOpErrorKind::UnknownWatch,
            ..
        }
    ));
    assert!(matches!(
        file_op(&state, &a, json!({"op": "stat", "path": "/x"})).await?,
        FileOpResponse::Unsupported { .. }
    ));
    Ok(())
}
