use std::sync::Arc;

use async_nats::jetstream;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_credential_broker::CommittedPolicyWriter;
use trogon_sandbox_oci::{OciClient, OciClientConfig};
use trogon_std::env::ReadEnv;

use super::durable_stores::DurableStores;
use super::lease_stores::LeaseStores;
use super::serve_error::ServeError;
use super::tenant_policies::TenantPolicies;
use super::worker_registry_setup;
use crate::bus::BrokerBus;
use crate::cli::{Cli, QuotaBackend};
use crate::digest_pinning::{BusSnapshotPinStore, SnapshotPinStore};
use crate::metrics::BrokerMetrics;
use crate::nats_bus::NatsBus;
use crate::signing_bus;
use crate::snapshot_tracker::{LiveSnapshotStore, NatsKvLiveSnapshotStore, RegistrySnapshotRemover};
use crate::worker_registry::WorkerRegistry;

/// Clients and stores shared by the broker core, its routes and its background loops.
pub(super) struct BrokerServices {
    pub(super) nats: async_nats::Client,
    pub(super) jetstream: jetstream::Context,
    pub(super) metrics: BrokerMetrics,
    pub(super) bus: Arc<dyn BrokerBus>,
    pub(super) leases: LeaseStores,
    pub(super) policies: TenantPolicies,
    pub(super) durable: DurableStores,
    pub(super) worker_registry: Arc<dyn WorkerRegistry>,
    pub(super) snapshot_pins: Arc<dyn SnapshotPinStore>,
    pub(super) live_snapshots: Arc<dyn LiveSnapshotStore>,
    pub(super) oci: Arc<OciClient>,
    pub(super) committed_policies: Option<Arc<dyn CommittedPolicyWriter>>,
}

impl BrokerServices {
    /// Opens every store the broker needs. Loops that keep a store fresh stop once `cancel` fires.
    pub(super) async fn open(
        cli: &Cli,
        env: &impl ReadEnv,
        nats: async_nats::Client,
        metrics: BrokerMetrics,
        cancel: CancellationToken,
    ) -> Result<Self, ServeError> {
        let jetstream = jetstream::new(nats.clone());
        let leases = LeaseStores::open(cli, &jetstream).await?;
        let policies = TenantPolicies::open(cli, &jetstream).await?;
        let worker_registry = worker_registry_setup::open(cli, &nats, &jetstream, cancel).await?;
        let bus = signing_bus::wrap_from_env(env, NatsBus::new(nats.clone()), &leases.active_leases)?;
        let snapshot_pins: Arc<dyn SnapshotPinStore> = BusSnapshotPinStore::new(Arc::clone(&bus));
        let live_snapshots: Arc<dyn LiveSnapshotStore> = NatsKvLiveSnapshotStore::open(jetstream.clone())
            .await
            .map_err(ServeError::opening("live snapshot store"))?;
        let oci = Arc::new(OciClient::new(OciClientConfig {
            insecure_registries: vec![],
            ..Default::default()
        }));
        let durable = DurableStores::open(&jetstream, cli.jetstream_replicas).await?;
        let committed_policies = cli
            .committed_policies
            .writer(&nats, matches!(cli.quota_backend, QuotaBackend::NatsKv))
            .await?;
        Ok(Self {
            nats,
            jetstream,
            metrics,
            bus,
            leases,
            policies,
            durable,
            worker_registry,
            snapshot_pins,
            live_snapshots,
            oci,
            committed_policies,
        })
    }

    /// Removes snapshots from `registry` and releases what they were charged.
    pub(super) fn snapshot_remover(&self, registry: &str) -> Result<Arc<RegistrySnapshotRemover>, ServeError> {
        let accounting = self
            .leases
            .quota
            .snapshot_store()
            .ok_or(ServeError::SnapshotAccountingUnavailable)?;
        Ok(RegistrySnapshotRemover::with_accounting(
            Arc::clone(&self.live_snapshots),
            Arc::clone(&self.oci),
            registry.to_owned(),
            accounting,
        ))
    }
}
