use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trogon_sandbox_oci::OciClient;
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode};
use trogon_sandbox_ssh_gateway::{BridgeLimits, HubSshHost, SshStreamBridge, TerminalHub};
use trogon_std::env::SystemEnv;

use crate::{
    GuestExec, KubeVirtGuestAccess, KubeVirtProvider, KubeVirtWorker, RusshGuestShell, S3RestoreStores,
    ServeWorkerError, SnapshotService, SshSessionKeys, WorkerArgs, run_get_domain_handler, run_ssh_handlers,
    run_terminal_handler,
};

/// Connects to NATS and the cluster and serves leases until `cancel` fires.
pub async fn serve_worker(args: &WorkerArgs, cancel: CancellationToken) -> Result<(), ServeWorkerError> {
    let nats = async_nats::connect(&args.nats_url)
        .await
        .map_err(|source| ServeWorkerError::Connect {
            url: args.nats_url.clone(),
            source,
        })?;
    let kube = kube::Client::try_default().await?;
    let config = args.kube_virt_config()?;
    let namespace = config.namespace.clone();
    let worker_id = config.worker_id.clone();
    let snapshots = Arc::new(SnapshotService::new(
        Arc::new(OciClient::new(config.oci.clone())),
        Arc::new(S3RestoreStores::new(SystemEnv)),
    ));
    let provider = Arc::new(
        KubeVirtProvider::new(config, kube, &args.state_dir, snapshots)?
            .with_max_concurrent_leases(args.max_concurrent_leases),
    );
    provider.recover_leases().await?;

    let access = Arc::new(KubeVirtGuestAccess::new(
        provider.clone(),
        GuestExec::default(),
        namespace,
    ));
    let keys = Arc::new(SshSessionKeys::new(access.clone()));
    provider.bind_ssh_keys(keys.clone())?;
    let terminals = Arc::new(TerminalHub::new(Arc::new(RusshGuestShell::new(access)), nats.clone()));
    let bridge = SshStreamBridge::new(
        Arc::new(HubSshHost(terminals.clone())),
        nats.clone(),
        BridgeLimits::default(),
    );

    let client = nats.clone();
    tokio::spawn(async move {
        if let Err(error) = run_ssh_handlers(client, keys, bridge).await {
            warn!(%error, "ssh handler loop exited");
        }
    });
    let (client, hub) = (nats.clone(), terminals.clone());
    tokio::spawn(async move {
        if let Err(error) = run_terminal_handler(client, hub).await {
            warn!(%error, "terminal handler loop exited");
        }
    });
    let (client, domains) = (nats.clone(), provider.clone());
    tokio::spawn(async move {
        if let Err(error) = run_get_domain_handler(client, domains).await {
            warn!(%error, "get_domain handler loop exited");
        }
    });

    let authority = AuthorityConfig::new(AuthorityMode::from_env(&SystemEnv)?);
    info!("sandbox-kubevirt-worker starting");
    KubeVirtWorker::new(nats, provider, terminals, worker_id, authority)
        .run(cancel.cancelled_owned())
        .await?;
    Ok(())
}
