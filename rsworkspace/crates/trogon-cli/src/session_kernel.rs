//! Session Kernel integration stack for trogon-cli cross-runner switching.

use async_nats::jetstream::kv::Store as JsKvStore;
use trogon_nats::jetstream::NatsJetStreamClient;
use trogonai_capabilities::{CapabilityConfig, ProviderCertificationMatrix};
use trogonai_session_kernel::{
    EventLog, SessionKernel, SessionKernelConfig, SessionKernelFeatureFlags, SessionKernelOperationalPolicy,
    SessionKvLeaseFactory, SessionLeaseManager, SnapshotStore, UsageStore, provision_lease_store,
    provision_snapshot_store, provision_usage_store,
};
use trogonai_session_projection::{ContextTwinStore, ProjectionConfig, provision_context_twin_store};
use trogonai_switching::{RunnerBindingStore, SwitchingConfig};

/// Provisioned Session Kernel backends used by `CrossRunnerSwitcher`.
#[derive(Clone)]
pub struct SessionKernelStack {
    pub flags: SessionKernelFeatureFlags,
    pub policies: SessionKernelOperationalPolicy,
    pub kernel:
        SessionKernel<EventLog<NatsJetStreamClient, NatsJetStreamClient>, JsKvStore, SessionKvLeaseFactory<JsKvStore>>,
    pub snapshots: SnapshotStore<JsKvStore>,
    pub runner_bindings: RunnerBindingStore<JsKvStore>,
    pub twin_store: ContextTwinStore<JsKvStore>,
    pub switching_config: SwitchingConfig,
    pub projection_config: ProjectionConfig,
    pub capability_config: CapabilityConfig,
    pub certification: ProviderCertificationMatrix,
}

impl SessionKernelStack {
    /// Provision NATS-backed kernel stores. Safe to call when flags are disabled
    /// (used for shadow mode without canonical binding).
    pub async fn provision(
        nats: async_nats::Client,
        flags: SessionKernelFeatureFlags,
        policies: SessionKernelOperationalPolicy,
    ) -> Result<Self, String> {
        let kernel_config = SessionKernelConfig::default();
        let projection_config = ProjectionConfig::default();
        let js = async_nats::jetstream::new(nats);
        let js_client = NatsJetStreamClient::new(js.clone());

        let snapshot_kv = provision_snapshot_store(&js, &kernel_config)
            .await
            .map_err(|err| err.to_string())?;
        let lease_kv = provision_lease_store(&js, &kernel_config)
            .await
            .map_err(|err| err.to_string())?;
        let usage_kv = provision_usage_store(&js, &kernel_config)
            .await
            .map_err(|err| err.to_string())?;
        let twin_kv = provision_context_twin_store(&js, &projection_config)
            .await
            .map_err(|err| err.to_string())?;
        let switching_config = switching_config_from_flags(&flags);
        let binding_kv = trogonai_switching::provision_runner_binding_store(&js, &switching_config)
            .await
            .map_err(|err| err.to_string())?;

        let event_log = EventLog::new(js_client.clone(), js_client, kernel_config.clone());
        // Provision the append-only event-log stream (§3; § NATS Operational Policy);
        // idempotent via get_or_create_stream.
        event_log
            .provision_stream(&NatsJetStreamClient::new(js.clone()), &policies.nats)
            .await
            .map_err(|err| err.to_string())?;
        let snapshots = SnapshotStore::new(snapshot_kv.clone(), kernel_config.clone());
        let leases = SessionLeaseManager::new(SessionKvLeaseFactory::new(lease_kv, &kernel_config), "trogon-cli");
        let usage = UsageStore::new(usage_kv, kernel_config.clone());
        let kernel = SessionKernel::new(kernel_config, event_log, snapshots.clone(), leases).with_usage_store(usage);
        let runner_bindings = RunnerBindingStore::new(binding_kv, switching_config.clone());
        let twin_store = ContextTwinStore::new(twin_kv, projection_config.clone());

        Ok(Self {
            flags,
            policies,
            kernel,
            snapshots,
            runner_bindings,
            twin_store,
            switching_config,
            projection_config,
            capability_config: CapabilityConfig::default(),
            // Initial certification matrix (cambio-modelo.md): the Switch Safety
            // Gate uses it to allow/warn cross-provider switches. An empty matrix
            // would treat every model as Experimental.
            certification: ProviderCertificationMatrix::baseline(),
        })
    }
}

pub fn switching_config_from_flags(flags: &SessionKernelFeatureFlags) -> SwitchingConfig {
    let base = SwitchingConfig::default();
    SwitchingConfig {
        switch_safety_gate_enabled: flags.safety_gate_enabled(),
        continuity_checkpoint_enabled: flags.checkpoint_enabled(),
        continuity_checkpoint_internal_echo: base.continuity_checkpoint_internal_echo,
        ..base
    }
}
