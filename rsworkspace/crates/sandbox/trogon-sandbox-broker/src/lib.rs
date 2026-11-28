#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        acyclic_modules,
        reason = "the broker core is one aggregate split across mutually recursive modules"
    )
)]

pub mod active_lease;
pub mod artifact_lister;
pub mod auth;
pub mod auth_setup;
pub mod bus;
pub mod cli;
pub mod command_launch;
mod command_launch_error;
mod command_log_error;
mod command_log_retention;
pub mod command_logs;
pub mod committed_policies;
mod constants;
pub mod core;
pub mod default_capability_profile;
pub mod digest_pinning;
pub mod drive_purge;
mod drive_remote_mounts;
pub mod drive_store;
mod entity_event_sink_error;
pub mod entity_shadow;
pub mod entity_store;
mod entity_store_error;
pub mod freeze_admin;
mod generated_ulid;
pub mod hardening_override_policy;
pub mod host_import_policy;
pub mod host_mount_policy;
pub mod idempotency;
pub mod idle;
mod image_labels;
pub mod image_policy;
pub mod image_readiness;
mod lease_extension;
pub mod limits;
mod log_page;
pub mod metrics;
pub mod named_lease;
pub mod native_policy;
pub mod native_snapshot;
pub mod nats_bus;
pub mod network_policy_validation;
pub mod operations;
pub mod pending_purge;
pub mod pending_release;
mod placement_gate;
pub mod prewarm;
mod prewarm_error;
mod prewarm_store_error;
mod profile_satisfaction;
pub mod quota;
pub mod resolved_digest;
pub mod retention_sweep;
mod sandbox_entity_mode;
mod sandbox_entity_mode_error;
pub mod serve;
pub mod signing_bus;
pub mod snapshot_accounting;
pub mod snapshot_capture_status;
pub mod snapshot_catalog;
pub mod snapshot_last_used;
mod snapshot_live;
pub mod snapshot_location;
pub mod snapshot_tag;
pub mod snapshot_tracker;
pub mod sweeper;
pub mod tenant_record;
pub mod usage_store;
pub mod worker_authority;
pub mod worker_fence;
pub mod worker_registry;

#[cfg(test)]
mod test_support;
