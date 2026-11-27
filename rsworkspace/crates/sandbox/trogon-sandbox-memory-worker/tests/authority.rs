//! Lease cancellation and the worker authority protocol against a real NATS server.

mod common;
mod nats_common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use common::{Harness, TestResult};
use futures::StreamExt;
use serde_json::json;
use tokio::sync::Notify;
use trogon_sandbox_contract::{
    CommandOutput, CommandTimeoutStatus, KillCommandResponse, LeaseId, OwnedLease, PollCommandResponse, ProviderKind,
    RunCommandResponse, WorkerAuthorityRequest, WorkerAuthorityResponse, WorkerId,
};
use trogon_sandbox_memory_worker::{CommandCtx, CommandHandler, CommandTable, MemoryWorker, MemoryWorkerConfig};
use trogon_sandbox_protocol::subjects::provider_cancel;
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityMode, AuthorityTaskScope, FreezeOutcome, HeldLease, PurgeOutcome,
};

use crate::nats_common::Server;

#[derive(Default)]
struct Control {
    started: Notify,
    release: Notify,
    stopped: AtomicUsize,
}

impl Control {
    fn stopped(&self) -> usize {
        self.stopped.load(Ordering::SeqCst)
    }
}

struct Stopped(Arc<Control>);

impl Drop for Stopped {
    fn drop(&mut self) {
        self.0.stopped.fetch_add(1, Ordering::SeqCst);
    }
}

/// Runs until its cancellation token fires or the test releases it.
struct Held(Arc<Control>);

#[async_trait]
impl CommandHandler for Held {
    async fn run(&self, ctx: CommandCtx) -> CommandOutput {
        let _stopped = Stopped(Arc::clone(&self.0));
        self.0.started.notify_one();
        tokio::select! {
            _ = ctx.cancel.cancelled() => {},
            _ = self.0.release.notified() => {},
        }
        CommandOutput::default()
    }
}

/// Never looks at its cancellation token, so only a forced stop ends it.
struct IgnoresCancellation(Arc<Control>);

#[async_trait]
impl CommandHandler for IgnoresCancellation {
    async fn run(&self, _ctx: CommandCtx) -> CommandOutput {
        let _stopped = Stopped(Arc::clone(&self.0));
        self.0.started.notify_one();
        self.0.release.notified().await;
        CommandOutput::default()
    }
}

async fn start_worker(
    commands: CommandTable,
    worker_id: &str,
    authority: AuthorityConfig,
) -> Result<Harness, Box<dyn std::error::Error>> {
    let server = Server::start().await;
    let client = server.client().await?;
    let worker = MemoryWorker::start_with_authority(
        client.clone(),
        MemoryWorkerConfig {
            commands,
            ..Default::default()
        },
        WorkerId::new(worker_id)?,
        authority,
    )
    .await?;
    Ok(Harness { server, client, worker })
}

fn held_commands(name: &str, handler: impl CommandHandler + 'static) -> CommandTable {
    let mut commands = CommandTable::empty();
    commands.register(name, handler);
    commands
}

async fn until(what: &str, mut done: impl FnMut() -> bool) -> TestResult {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .map_err(|_| format!("timed out waiting until {what}"))?;
    Ok(())
}

async fn started(control: &Control) -> TestResult {
    tokio::time::timeout(Duration::from_secs(5), control.started.notified())
        .await
        .map_err(|_| "the command never started")?;
    Ok(())
}

async fn cancel(h: &Harness, lease: &str) -> TestResult {
    h.client
        .publish(provider_cancel(ProviderKind::Mock, &LeaseId::new(lease)), "{}".into())
        .await?;
    h.client.flush().await?;
    until("the lease was forgotten", || h.worker.lease_count() == 0).await
}

async fn launch(
    h: &Harness,
    lease: &str,
    command: &str,
    detached: bool,
) -> Result<RunCommandResponse, Box<dyn std::error::Error>> {
    h.call(
        "run_command",
        lease,
        json!({"lease_id": lease, "command": [command], "detached": detached, "timeout_seconds": 60}),
    )
    .await
}

fn last_held(h: &Harness) -> Result<HeldLease, Box<dyn std::error::Error>> {
    h.worker
        .authority_held_leases()
        .pop()
        .ok_or_else(|| "no lease is held".into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_lease_ends_its_detached_command_before_forgetting_it() -> TestResult {
    let control = Arc::new(Control::default());
    let h = start_worker(
        held_commands("held", Held(Arc::clone(&control))),
        "cancel-detached",
        AuthorityConfig::new(AuthorityMode::Off),
    )
    .await?;
    h.grant("held-command").await?;
    assert!(matches!(
        launch(&h, "held-command", "held", true).await?,
        RunCommandResponse::Started(_)
    ));
    started(&control).await?;
    cancel(&h, "held-command").await?;
    // Removing the lease from the map is not enough: the task itself has ended.
    let stopped_before_forgetting = control.stopped();
    control.release.notify_waiters();
    h.worker.shutdown().await;
    assert_eq!(stopped_before_forgetting, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_lease_joins_its_attached_command_before_forgetting_it() -> TestResult {
    let control = Arc::new(Control::default());
    let h = Arc::new(
        start_worker(
            held_commands("held", Held(Arc::clone(&control))),
            "cancel-attached",
            AuthorityConfig::new(AuthorityMode::Off),
        )
        .await?,
    );
    h.grant("attached-held").await?;
    let running = tokio::spawn({
        let h = Arc::clone(&h);
        async move {
            launch(&h, "attached-held", "held", false)
                .await
                .map_err(|error| error.to_string())
        }
    });
    started(&control).await?;
    cancel(&h, "attached-held").await?;
    let stopped = control.stopped();
    control.release.notify_waiters();
    assert!(matches!(running.await??, RunCommandResponse::Completed(_)));
    let h = Arc::into_inner(h).ok_or("the harness is still shared")?;
    h.worker.shutdown().await;
    assert_eq!(stopped, 1);
    Ok(())
}

#[tokio::test]
async fn the_grant_and_the_heartbeat_share_the_worker_identity_and_live_lease_count() -> TestResult {
    let h = start_worker(
        CommandTable::default(),
        "memory-identity",
        AuthorityConfig::new(AuthorityMode::Off),
    )
    .await?;
    let heartbeat = h.worker.heartbeat_source(8, vec![]);
    assert_eq!(heartbeat().capacity.current_leases, 0);
    // `grant` checks that the reply names this worker.
    h.grant("identity-lease").await?;
    assert_eq!(h.worker.worker_id().as_str(), "memory-identity");
    assert_eq!(heartbeat().worker_id.as_str(), "memory-identity");
    assert_eq!(heartbeat().capacity.current_leases, 1);
    let original = h.worker.authority_held_leases();
    h.grant("identity-lease").await?;
    // A redispatch keeps the resource it already holds.
    assert_eq!(h.worker.authority_held_leases(), original);
    cancel(&h, "identity-lease").await?;
    assert_eq!(heartbeat().capacity.current_leases, 0);
    h.worker.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn neither_a_settled_resource_nor_a_foreign_completion_can_purge_a_reused_lease_id() -> TestResult {
    let h = start_worker(
        CommandTable::default(),
        "memory-reuse",
        AuthorityConfig::new(AuthorityMode::Off),
    )
    .await?;
    let id = LeaseId::new("reused-id");
    h.grant(id.as_str()).await?;
    let old = last_held(&h)?;
    cancel(&h, id.as_str()).await?;
    h.grant(id.as_str()).await?;
    let replacement = last_held(&h)?;
    assert_ne!(old, replacement);
    assert_eq!(h.worker.authority_purge(&old).await, PurgeOutcome::Settled);

    let HeldLease::Provider { resource_id, .. } = &old else {
        return Err("the memory worker holds provider resources".into());
    };
    let foreign = AuthorityTaskScope::default();
    let (forged, completion) = foreign.track(id.clone(), resource_id.as_str())?;
    completion.confirm();
    let forged = HeldLease::Provider {
        lease_id: id.clone(),
        provider: ProviderKind::Mock,
        resource_id: forged,
    };
    assert_eq!(h.worker.authority_purge(&forged).await, PurgeOutcome::Stale);
    assert_eq!(
        h.worker.authority_freeze(&replacement).await,
        FreezeOutcome::Unsupported
    );
    assert_eq!(h.worker.authority_held_leases(), vec![replacement]);
    assert_eq!(h.worker.lease_count(), 1);
    h.worker.shutdown().await;
    Ok(())
}

fn answer(request: WorkerAuthorityRequest, transferred: bool) -> WorkerAuthorityResponse {
    if transferred {
        WorkerAuthorityResponse {
            owned: vec![],
            not_assigned: request.lease_ids,
        }
    } else {
        WorkerAuthorityResponse {
            owned: request
                .lease_ids
                .into_iter()
                .map(|lease_id| OwnedLease { lease_id, epoch: None })
                .collect(),
            not_assigned: vec![],
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transferred_lease_is_settled_only_after_its_command_ended() -> TestResult {
    let server = Server::start().await;
    let client = server.client().await?;
    let transferred = Arc::new(AtomicBool::new(false));
    let mut queries = client.subscribe("sandbox.worker.authority.*").await?;
    client.flush().await?;
    let oracle = tokio::spawn({
        let (client, transferred) = (client.clone(), Arc::clone(&transferred));
        async move {
            while let Some(message) = queries.next().await {
                let (Some(reply), Ok(request)) = (
                    message.reply,
                    serde_json::from_slice::<WorkerAuthorityRequest>(&message.payload),
                ) else {
                    continue;
                };
                if request.worker_id.as_str() != "memory-guard" {
                    continue;
                }
                let response = answer(request, transferred.load(Ordering::SeqCst));
                if let Ok(body) = serde_json::to_vec(&response) {
                    let _ = client.publish(reply, body.into()).await;
                }
            }
        }
    });

    let mut authority = AuthorityConfig::new(AuthorityMode::Enforce);
    authority.interval = Duration::from_millis(10);
    authority.min_lease_age = Duration::ZERO;
    authority.confirmations = 1;
    authority.grace = Duration::ZERO;
    let control = Arc::new(Control::default());
    let worker = MemoryWorker::start_with_authority(
        client.clone(),
        MemoryWorkerConfig {
            commands: held_commands("held", Held(Arc::clone(&control))),
            ..Default::default()
        },
        WorkerId::new("memory-guard")?,
        authority,
    )
    .await?;
    let h = Harness { server, client, worker };
    h.grant("guard-held").await?;
    assert!(matches!(
        launch(&h, "guard-held", "held", true).await?,
        RunCommandResponse::Started(_)
    ));
    started(&control).await?;
    transferred.store(true, Ordering::SeqCst);
    let settled = until("the guard reclaimed the lease", || h.worker.lease_count() == 0).await;
    let stopped = control.stopped();
    control.release.notify_waiters();
    h.worker.shutdown().await;
    oracle.abort();
    settled?;
    assert_eq!(stopped, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sigkill_ends_a_handler_that_ignores_cancellation_before_reporting_signaled() -> TestResult {
    let control = Arc::new(Control::default());
    let h = start_worker(
        held_commands("uncooperative", IgnoresCancellation(Arc::clone(&control))),
        "memory-sigkill",
        AuthorityConfig::new(AuthorityMode::Off),
    )
    .await?;
    h.grant("direct-sigkill").await?;
    let RunCommandResponse::Started(started_command) = launch(&h, "direct-sigkill", "uncooperative", true).await?
    else {
        return Err("the handler must start".into());
    };
    started(&control).await?;
    let kill = json!({"lease_id": "direct-sigkill", "execution_id": started_command.execution_id, "signal": "kill"});
    assert_eq!(
        h.call::<KillCommandResponse>("kill_command", "direct-sigkill", kill.clone())
            .await?,
        KillCommandResponse::Signaled
    );
    // A cooperative token alone could not stop this handler, so the reply came after it ended.
    let stopped_at_reply = control.stopped();
    let poll = h
        .call::<PollCommandResponse>(
            "poll_command",
            "direct-sigkill",
            json!({"lease_id": "direct-sigkill", "execution_id": started_command.execution_id}),
        )
        .await?;
    let repeated = h
        .call::<KillCommandResponse>("kill_command", "direct-sigkill", kill)
        .await?;
    control.release.notify_waiters();
    h.worker.shutdown().await;
    assert_eq!(stopped_at_reply, 1);
    assert!(matches!(
        poll,
        PollCommandResponse::Completed(CommandOutput {
            exit_code: 137,
            timed_out: CommandTimeoutStatus::WithinDeadline,
            ..
        })
    ));
    assert_eq!(repeated, KillCommandResponse::AlreadyCompleted);
    Ok(())
}
