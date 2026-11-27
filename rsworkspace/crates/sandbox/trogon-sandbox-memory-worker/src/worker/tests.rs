use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use trogon_sandbox_contract::{
    CommandOutput, ExecutionId, LeaseId, ProcessName, ProviderKind, ReadinessNeedle, ReadinessProbe,
    RestartCommandResponse, RunCommandRequest, RunCommandResponse, StopCommandResponse, UnixSignal, WaitReadyRequest,
    WaitReadyResponse, WorkerId,
};
use trogon_sandbox_protocol::{AuthorityHost, FreezeOutcome, HeldLease, PurgeOutcome, ThawOutcome};

use super::authority_host::MemoryAuthorityHost;
use super::command_launch::{run_command, signal_number};
use super::command_task::ClosePolicy;
use super::exec_state::ExecState;
use super::lease::Lease;
use super::lease_task::run_lease_task;
use super::proc_entry::Proc;
use super::process_control::{restart_command, stop_command};
use super::process_query::wait_ready;
use super::state::State;
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
            signal: trogon_sandbox_contract::StopSignal::Kill
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
