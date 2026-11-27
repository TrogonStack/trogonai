use prometheus::{IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry};

mod broker_metrics_error;

pub use broker_metrics_error::BrokerMetricsError;

#[derive(Clone)]
pub struct BrokerMetrics {
    registry: Registry,
    pub lease_dispatch_total: IntCounterVec,
    pub active_leases: IntGaugeVec,
    pub locality_hit_total: IntCounterVec,
    pub digest_pin_mismatch_total: IntCounter,
    pub unsettled_freezes: IntGauge,
}

impl BrokerMetrics {
    pub fn new() -> Result<Self, BrokerMetricsError> {
        let registry = Registry::new();

        let lease_dispatch_total = IntCounterVec::new(
            Opts::new(
                "sandbox_broker_lease_dispatch_total",
                "Lease dispatch attempts by tenant, provider, and outcome",
            ),
            &["tenant", "provider", "outcome"],
        )?;
        registry.register(Box::new(lease_dispatch_total.clone()))?;

        let active_leases = IntGaugeVec::new(
            Opts::new(
                "sandbox_broker_active_leases",
                "Active leases tracked by the broker per tenant",
            ),
            &["tenant"],
        )?;
        registry.register(Box::new(active_leases.clone()))?;

        let locality_hit_total = IntCounterVec::new(
            Opts::new(
                "sandbox_broker_locality_hit_total",
                "Lease dispatches that selected a worker with a warm cached digest",
            ),
            &["provider"],
        )?;
        registry.register(Box::new(locality_hit_total.clone()))?;

        let digest_pin_mismatch_total = IntCounter::new(
            "sandbox_broker_digest_pin_mismatch_total",
            "Snapshot restore digest pin mismatches detected",
        )?;
        registry.register(Box::new(digest_pin_mismatch_total.clone()))?;

        let unsettled_freezes = IntGauge::new(
            "sandbox_broker_unsettled_freezes",
            "Leases whose pause or resume the reconciler gave up settling",
        )?;
        registry.register(Box::new(unsettled_freezes.clone()))?;

        Ok(Self {
            registry,
            lease_dispatch_total,
            active_leases,
            locality_hit_total,
            digest_pin_mismatch_total,
            unsettled_freezes,
        })
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn gather_text(&self) -> Result<String, trogon_sandbox_observability::ObservabilityError> {
        trogon_sandbox_observability::gather_text(&self.registry)
    }

    pub fn record_dispatch(&self, tenant: &str, provider: trogon_sandbox_contract::ProviderKind, outcome: &str) {
        let provider = trogon_sandbox_protocol::subjects::provider_token(provider);
        self.lease_dispatch_total
            .with_label_values(&[tenant, provider, outcome])
            .inc();
    }

    pub fn inc_active_leases(&self, tenant: &str) {
        self.active_leases.with_label_values(&[tenant]).inc();
    }

    pub fn dec_active_leases(&self, tenant: &str) {
        self.active_leases.with_label_values(&[tenant]).dec();
    }
}

#[cfg(test)]
mod tests;
