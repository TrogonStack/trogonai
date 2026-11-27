use std::sync::Arc;
use std::time::Duration;

use super::broker_services::BrokerServices;
use super::serve_error::ServeError;
use crate::cli::Cli;
use crate::core::{BrokerConfig, BrokerCore, LifetimePolicy};
use crate::drive_purge::S3DriveDataPurger;
use crate::hardening_override_policy::HardeningOverridePolicy;
use crate::host_import_policy::InMemoryHostImportPolicy;
use crate::native_policy::StaticThenStoredNativePassthrough;

/// Builds the broker core from the opened stores and the configured limits.
pub(super) fn assemble(cli: &Cli, services: &BrokerServices) -> Result<BrokerCore, ServeError> {
    let durable = &services.durable;
    let broker = BrokerCore::new(
        Arc::clone(&services.bus),
        Arc::clone(&services.leases.quota),
        Arc::clone(&services.leases.idempotency),
        Arc::clone(&services.leases.active_leases),
        Arc::clone(&services.policies.images),
        Arc::clone(&services.policies.host_mounts),
        Arc::new(InMemoryHostImportPolicy::new()),
        Arc::clone(&services.policies.limits),
        Arc::clone(&services.worker_registry),
        Arc::clone(&services.snapshot_pins),
        Arc::clone(&durable.tags),
        Arc::clone(&durable.resolved_digests),
        Arc::clone(&durable.drives),
        Arc::clone(&durable.named_leases),
        config(cli),
        services.metrics.clone(),
    );
    Ok(broker
        .with_max_command_output_bytes(cli.max_command_output_bytes)
        .with_orphan_drive_claim_grace(Duration::from_secs(cli.orphan_drive_claim_grace_secs))
        .with_command_launch_store(Arc::clone(&services.leases.command_launches))
        .with_lifetime_policy(LifetimePolicy {
            max_lease_lifetime_seconds: cli.max_lease_lifetime_seconds,
            allow_no_ttl: cli.allow_no_ttl_leases,
        })
        .with_hardening_override_policy(HardeningOverridePolicy::allowing(
            cli.hardening_override_tenants.iter().cloned(),
        ))
        .with_snapshot_last_used(Arc::clone(&durable.snapshot_last_used))
        .with_native_snapshot_store(Arc::clone(&durable.native_snapshots))
        .with_usage_store(Arc::clone(&durable.usage))
        .with_drive_data_purger(Arc::new(S3DriveDataPurger))
        .with_pending_purge_store(Arc::clone(&durable.pending_purges))
        .with_pending_release_store(Arc::clone(&durable.pending_releases))
        .with_committed_policy_writer_if(services.committed_policies.clone())
        .with_snapshot_kv_reader(Arc::clone(&durable.snapshot_kv_reader))
        .with_snapshot_manifest_probe(Arc::clone(&services.oci) as _)
        .with_snapshot_remover(services.snapshot_remover(&cli.snapshot_registry)?)
        .with_snapshot_tag_history_reader(Arc::clone(&durable.tag_history_reader))
        .with_native_policy(Arc::new(StaticThenStoredNativePassthrough::new(
            cli.native_passthrough_tenants.iter().cloned(),
            Arc::clone(&services.policies.native_passthrough),
        ))))
}

fn config(cli: &Cli) -> BrokerConfig {
    BrokerConfig {
        default_cpu_cap: cli.default_cpu_cap,
        default_wall_cap: cli.default_wall_cap,
        enforce_image_allowlist: cli.enforce_image_allowlist,
        enforce_worker_registry: cli.enforce_worker_registry,
        default_max_cpu_per_request: cli.default_max_cpu_seconds_per_request,
        default_max_memory_mib_per_request: cli.default_max_memory_mib_per_request,
        default_active_lease_cap: cli.default_active_lease_cap,
        snapshot_registry: cli.snapshot_registry.clone(),
        provider_dispatch_timeout: Duration::from_secs(cli.provider_dispatch_timeout_secs),
    }
}
