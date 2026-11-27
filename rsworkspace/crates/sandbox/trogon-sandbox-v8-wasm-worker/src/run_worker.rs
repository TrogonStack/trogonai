use std::sync::Arc;

use async_nats::Client;
use futures::StreamExt;
use futures::stream::{BoxStream, select_all};
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_sandbox_contract::{LeaseId, ProviderKind, WorkerId};
use trogon_sandbox_protocol::subjects::{provider_cancel, provider_dispatch};
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, AuthorityMode, NatsAuthorityOracle, Provider, RedispatchGuard,
    queue_subscribe_verified, subscribe_verified,
};
use trogon_std::env::SystemEnv;

use crate::cancel_dispatch::handle_cancel;
use crate::constants::LEASE_QUEUE_GROUP;
use crate::lease_dispatch::handle_lease;
use crate::routed_reply::handle_verb;
use crate::routing::Routing;
use crate::worker_event::WorkerEvent;
use crate::{RoutedVerb, V8WasmProvider, WorkerError};

/// Serves lease dispatches, cancels and routed verbs for `worker` until `cancel` fires, then stops
/// every guest `v8` still holds.
///
/// `provider` is what dispatched leases run on. It is normally `v8` itself; a different provider
/// has its leases tracked on `v8` so the routed verbs still find them.
pub async fn run_worker(
    client: Client,
    provider: Arc<dyn Provider>,
    v8: Arc<V8WasmProvider>,
    worker: WorkerId,
    cancel: CancellationToken,
) -> Result<(), WorkerError> {
    v8.bind_worker(&worker)?;
    let native: Arc<dyn Provider> = v8.clone();
    let routing = Arc::new(Routing {
        host: v8.clone(),
        worker: worker.clone(),
        external: !Arc::ptr_eq(&provider, &native),
    });
    let authority = AuthorityGuard::new(
        AuthorityConfig::new(AuthorityMode::from_env(&SystemEnv)?),
        worker.clone(),
        Arc::new(NatsAuthorityOracle::new(client.clone(), worker)),
        v8.clone(),
    );
    tokio::spawn(authority.run(cancel.clone()));

    let lease_subject = provider_dispatch(ProviderKind::V8Wasm);
    let cancel_subject = provider_cancel(ProviderKind::V8Wasm, &LeaseId::new(">"));
    let mut streams: Vec<BoxStream<'static, WorkerEvent>> = vec![
        queue_subscribe_verified(&client, lease_subject.clone(), LEASE_QUEUE_GROUP)
            .await?
            .map(WorkerEvent::Lease)
            .boxed(),
        subscribe_verified(&client, cancel_subject.clone())
            .await?
            .map(WorkerEvent::Cancel)
            .boxed(),
    ];
    for verb in RoutedVerb::ALL {
        streams.push(
            subscribe_verified(&client, verb.subject())
                .await?
                .map(move |msg| WorkerEvent::Verb(verb, msg))
                .boxed(),
        );
    }
    let mut events = select_all(streams);
    let grants = Arc::new(RedispatchGuard::new());
    info!(%lease_subject, %cancel_subject, "worker ready");

    while let Some(event) = tokio::select! {
        _ = cancel.cancelled() => None,
        event = events.next() => event,
    } {
        match event {
            WorkerEvent::Lease(msg) => {
                tokio::spawn(handle_lease(
                    msg,
                    client.clone(),
                    provider.clone(),
                    grants.clone(),
                    routing.clone(),
                ));
            }
            WorkerEvent::Cancel(msg) => {
                tokio::spawn(handle_cancel(msg, provider.clone(), grants.clone(), routing.clone()));
            }
            WorkerEvent::Verb(verb, msg) => {
                tokio::spawn(handle_verb(client.clone(), v8.clone(), verb, msg));
            }
        }
    }
    info!("worker shutting down");
    Ok(v8.shutdown().await?)
}
