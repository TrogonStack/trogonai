mod cancel_dispatch;
mod command_launches;
mod constants;
mod e2b_worker_error;
mod lease_dispatch;
mod lease_ref;
mod lease_verbs;
mod log_exit;
mod native_snapshot_delete;
mod owned_cleanup;
mod reply;
mod respond_if_held;
mod snapshot_cleanup;
mod snapshot_export;
mod targeted;
mod targeted_cleanup;
mod targeted_dispatch;

use std::sync::Arc;

use async_nats::Client;
use async_nats::jetstream;
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_sandbox_contract::WorkerId;
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, NatsAuthorityOracle, Provider, RedispatchGuard, queue_subscribe_verified,
    subscribe_verified,
};
use trogon_sandbox_ssh_gateway::{BridgeLimits, SshStreamBridge};

use crate::E2bWorkerProvider;
use crate::terminal::{E2bSshHost, E2bTerminals};
use cancel_dispatch::handle_cancel_msg;
use command_launches::serve_command_launches;
use lease_dispatch::{ActiveLeases, handle_lease_msg};
use lease_verbs::serve_lease_verbs;
use native_snapshot_delete::serve_native_snapshot_delete;
use owned_cleanup::serve_owned_cleanup;
use snapshot_cleanup::spawn_snapshot_cleanup;
use snapshot_export::serve_snapshot_export;
use targeted::serve_targeted;

pub use e2b_worker_error::E2bWorkerError;
pub(crate) use log_exit::log_exit;

/// Serves `e2b` over NATS as `worker_id` until `cancel` fires: lease dispatch and cancel, the assignments addressed to
/// this worker, the per-lease verbs, the SSH stream bridge, and the authority check that keeps the worker honest about
/// what it holds. `authority` says how strictly that check acts on a lease the broker disowns.
pub async fn run(
    client: Client,
    js: jetstream::Context,
    e2b: Arc<E2bWorkerProvider>,
    worker_id: &WorkerId,
    cancel: CancellationToken,
    terminals: Arc<E2bTerminals>,
    authority: AuthorityConfig,
) -> Result<(), E2bWorkerError> {
    e2b.bind_worker(worker_id)?;
    e2b.configure_snapshots(js.clone())?;
    e2b.initialize_dispatch(js.clone()).await?;
    let provider: Arc<dyn Provider> = e2b.clone();
    serve_targeted(&client, &js, &e2b, &cancel).await?;
    serve_owned_cleanup(&client, &e2b, &cancel).await?;
    serve_native_snapshot_delete(&client, &e2b, &cancel).await?;
    spawn_snapshot_cleanup(Arc::clone(&e2b), cancel.clone());
    serve_command_launches(&client, &e2b, &cancel).await?;
    serve_lease_verbs(&client, &e2b, &cancel).await?;
    // SSH sessions are terminals on the same PTYs.
    let bridge = SshStreamBridge::new(
        Arc::new(E2bSshHost(Arc::clone(&terminals))),
        client.clone(),
        BridgeLimits::default(),
    );
    tokio::spawn({
        let (client, cancel) = (client.clone(), cancel.clone());
        async move {
            log_exit("ssh stream bridge", bridge.serve(client, cancel).await);
        }
    });
    serve_snapshot_export(&client, worker_id, &cancel).await?;
    let mut leases = queue_subscribe_verified(&client, constants::LEASE, constants::LEASE_QUEUE_GROUP).await?;
    let mut cancels = subscribe_verified(&client, constants::CANCEL).await?;
    let active: ActiveLeases = Arc::new(Mutex::new(Default::default()));
    let grants = Arc::new(RedispatchGuard::new());
    let guard = AuthorityGuard::new(
        authority,
        worker_id.clone(),
        Arc::new(NatsAuthorityOracle::new(client.clone(), worker_id.clone())),
        e2b.clone(),
    );
    tokio::spawn(guard.run(cancel.clone()));
    client.flush().await?;
    e2b.mark_ready();
    info!("worker ready");
    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                info!("worker shutting down");
                break;
            }
            next = leases.next() => {
                let Some(message) = next else { break };
                let (client, js, provider) = (client.clone(), js.clone(), Arc::clone(&provider));
                let (active, grants) = (Arc::clone(&active), Arc::clone(&grants));
                tokio::spawn(async move {
                    log_exit(
                        "lease request",
                        handle_lease_msg(message, client, js, provider, active, grants).await,
                    );
                });
            }
            next = cancels.next() => {
                let Some(message) = next else { break };
                let (provider, active, grants) = (Arc::clone(&provider), Arc::clone(&active), Arc::clone(&grants));
                tokio::spawn(handle_cancel_msg(message, provider, active, grants, Arc::clone(&terminals)));
            }
        }
    }
    Ok(())
}
