use std::sync::Arc;

use async_nats::Client;
use futures::StreamExt;
use futures::stream::{BoxStream, select_all};
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_sandbox_contract::{LeaseId, ProviderKind, WorkerId};
use trogon_sandbox_protocol::subjects::{provider_cancel, provider_dispatch, worker_snapshot_export};
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, AuthorityMode, NatsAuthorityOracle, Provider, RedispatchGuard,
    queue_subscribe_verified, subscribe_verified, terminal_replay_bytes_from_env,
};
use trogon_sandbox_ssh_gateway::{BridgeLimits, SshStreamBridge};
use trogon_std::env::SystemEnv;

use crate::cancel_dispatch::handle_cancel;
use crate::constants::LEASE_QUEUE_GROUP;
use crate::lease_dispatch::handle_lease;
use crate::snapshot_rpc::handle_snapshot_export;
use crate::verb_context::VerbContext;
use crate::verb_dispatch::handle_verb;
use crate::worker_event::WorkerEvent;
use crate::{K8sAuthorityHost, K8sSshHost, K8sTerminals, K8sVerb, K8sWorkerProvider, TerminalBackend, WorkerError};

/// Serves lease dispatches, cancels, snapshot exports and per-lease verbs for `worker` until `cancel` fires.
pub async fn run_worker(
    client: Client,
    provider: Arc<K8sWorkerProvider>,
    worker: WorkerId,
    cancel: CancellationToken,
) -> Result<(), WorkerError> {
    let grants = Arc::new(RedispatchGuard::new());
    let backend: Arc<dyn TerminalBackend> = provider.clone();
    let terminals = Arc::new(K8sTerminals::new(
        backend,
        client.clone(),
        terminal_replay_bytes_from_env(&SystemEnv),
    ));
    let bridge = SshStreamBridge::new(
        Arc::new(K8sSshHost(terminals.clone())),
        client.clone(),
        BridgeLimits::default(),
    );
    let authority = AuthorityGuard::new(
        AuthorityConfig::new(AuthorityMode::from_env(&SystemEnv)?),
        worker.clone(),
        Arc::new(NatsAuthorityOracle::new(client.clone(), worker.clone())),
        Arc::new(K8sAuthorityHost::new(
            provider.clone(),
            grants.clone(),
            terminals.clone(),
        )),
    );
    tokio::spawn(authority.run(cancel.child_token()));

    let lease_subject = provider_dispatch(ProviderKind::K8sMicroVm);
    let cancel_subject = provider_cancel(ProviderKind::K8sMicroVm, &LeaseId::new(">"));
    let mut streams: Vec<BoxStream<'static, WorkerEvent>> = vec![
        queue_subscribe_verified(&client, lease_subject.clone(), LEASE_QUEUE_GROUP)
            .await?
            .map(WorkerEvent::Lease)
            .boxed(),
        subscribe_verified(&client, cancel_subject.clone())
            .await?
            .map(WorkerEvent::Cancel)
            .boxed(),
        subscribe_verified(&client, worker_snapshot_export(&worker))
            .await?
            .map(WorkerEvent::SnapshotExport)
            .boxed(),
    ];
    for verb in K8sVerb::ALL {
        streams.push(
            subscribe_verified(&client, verb.subject())
                .await?
                .map(move |msg| WorkerEvent::Verb(verb, msg))
                .boxed(),
        );
    }
    let mut events = select_all(streams);
    let context = Arc::new(VerbContext {
        client: client.clone(),
        provider: provider.clone(),
        terminals: terminals.clone(),
        bridge,
    });
    let leases: Arc<dyn Provider> = provider.clone();
    info!(%lease_subject, %cancel_subject, "worker ready");

    while let Some(event) = tokio::select! {
        _ = cancel.cancelled() => None,
        event = events.next() => event,
    } {
        match event {
            WorkerEvent::Lease(msg) => {
                tokio::spawn(handle_lease(msg, client.clone(), leases.clone(), grants.clone()));
            }
            WorkerEvent::Cancel(msg) => {
                tokio::spawn(handle_cancel(msg, leases.clone(), grants.clone(), terminals.clone()));
            }
            WorkerEvent::SnapshotExport(msg) => {
                let (client, provider) = (client.clone(), provider.clone());
                tokio::spawn(async move { handle_snapshot_export(client, &provider, msg).await });
            }
            WorkerEvent::Verb(verb, msg) => {
                let context = context.clone();
                tokio::spawn(async move { handle_verb(&context, verb, msg).await });
            }
        }
    }
    info!("worker shutting down");
    Ok(())
}
