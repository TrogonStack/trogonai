use std::sync::Arc;

use async_nats::jetstream;
use tokio_util::sync::CancellationToken;
use tracing::info;
use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_cosign::CosignVerifier;
use trogon_sandbox_oci::{OciClient, OciClientConfig, RegistryHost};
use trogon_sandbox_protocol::{DEFAULT_HEARTBEAT_INTERVAL, spawn_heartbeat_loop};
use trogon_sandbox_providers::K8sMicroVmProvider;

use crate::constants::LEASE_SWEEP_INTERVAL;
use crate::{
    DigestCache, K8sWorkerProvider, OciImageUserResolver, ServeError, WarmPool, WorkerArgs, WorkerIdentity,
    build_heartbeat, profile_for, run_worker,
};

/// Connects to NATS and the cluster and serves leases as one worker replica until `cancel` fires.
pub async fn serve(args: &WorkerArgs, cancel: CancellationToken) -> Result<(), ServeError> {
    let nats = async_nats::connect(&args.nats_url)
        .await
        .map_err(|source| ServeError::Connect {
            url: args.nats_url.clone(),
            source,
        })?;
    let js = jetstream::new(nats.clone());
    let identity = args.worker_identity.clone().unwrap_or_else(WorkerIdentity::generate);
    let worker = WorkerId::new(identity.as_str())?;
    let k8s_config = args.k8s_config(&identity);
    let profile = profile_for(|tier| k8s_config.runtime_class_for(tier).to_owned());

    let insecure = args.insecure_registries();
    let oci = Arc::new(OciClient::new(OciClientConfig {
        insecure_registries: insecure.iter().map(RegistryHost::new).collect::<Result<Vec<_>, _>>()?,
        ..Default::default()
    }));
    let inner = Arc::new(
        K8sMicroVmProvider::with_default_client(k8s_config)
            .await?
            .with_violation_nats_url(args.nats_url.clone())
            .with_image_user_resolver(Arc::new(OciImageUserResolver::new(Arc::clone(&oci)))),
    );
    let cosign_verifier = args
        .cosign_public_key
        .as_ref()
        .map(|path| CosignVerifier::from_pem_path(path).map(|verifier| verifier.with_insecure_registries(insecure)))
        .transpose()?;
    let digest_cache = Arc::new(DigestCache::new(args.digest_cache_size));
    let warm_pool = Arc::new(WarmPool::new(
        Arc::clone(&inner),
        Arc::clone(&oci),
        Arc::clone(&digest_cache),
        args.warm_pool_config(),
        args.image.clone(),
        args.snapshot_registry.clone(),
        args.snapshot_repository.clone(),
        args.runtime_class.clone(),
    ));
    Arc::clone(&warm_pool).spawn_controller(cancel.clone());

    let provider = Arc::new(
        K8sWorkerProvider::new(
            inner,
            args.image.clone(),
            js,
            oci,
            cosign_verifier,
            Arc::clone(&digest_cache),
            Some(Arc::clone(&warm_pool)),
        )
        .with_persisted_rollback_state(args.persist_rollback_state),
    );

    // Before taking lease requests, so a lease launched now is never mixed up with one being recovered.
    let recovery = provider.recover_leases().await?;
    info!(
        cold = recovery.cold,
        warm = recovery.warm,
        swept = recovery.swept,
        "recovered leases"
    );
    Arc::clone(&provider).spawn_lease_sweeper(LEASE_SWEEP_INTERVAL, cancel.clone());

    let max_concurrent_leases = args.max_concurrent_leases;
    let (id, host) = (worker.clone(), Arc::clone(&provider));
    spawn_heartbeat_loop(
        nats.clone(),
        ProviderKind::K8sMicroVm,
        worker.clone(),
        DEFAULT_HEARTBEAT_INTERVAL,
        cancel.clone(),
        move || {
            let (id, host, profile) = (id.clone(), Arc::clone(&host), profile.clone());
            let (digest_cache, warm_pool) = (Arc::clone(&digest_cache), Arc::clone(&warm_pool));
            async move {
                let current = u32::try_from(host.active_lease_count().await).unwrap_or(u32::MAX);
                build_heartbeat(
                    &id,
                    max_concurrent_leases,
                    &profile,
                    current,
                    digest_cache.list(),
                    warm_pool.population().await,
                )
            }
        },
    );
    info!(max_concurrent_leases, "worker starting");
    let health = args.observability.health();
    health.started_with_nats(&nats);
    health.shutdown_when(cancel.clone().cancelled_owned());
    Ok(run_worker(nats, provider, worker, cancel).await?)
}
