//! Session Kernel: leases, append-only event log, snapshot materialization, and recovery.

pub mod config;
pub mod error;
pub mod event_log;
pub mod features;
pub mod kernel;
pub mod lease;
pub mod materialize;
pub mod migration;
pub mod nats;
pub mod policies;
pub mod recovery;
pub mod snapshot;
pub mod state;
pub mod telemetry;
pub mod usage;

pub use config::SessionKernelConfig;
pub use error::SessionKernelError;
pub use features::{
    EventLogPrimaryMode, RunnerBindingMode, SessionKernelFeatureFlags,
};
pub use migration::{
    compare_shadow_divergence, conversation_from_legacy_export, resolve_event_log_primary,
    shadow_sync_from_export, SessionMigrationRecord, ShadowDivergenceReport, ShadowSyncReport,
    snapshot_from_legacy_export,
};
pub use policies::{
    ContinuitySloPolicy, NatsOperationalPolicy, OperationalProductPolicy,
    SessionErrorUxState, SessionKernelOperationalPolicy,
};
pub use event_log::{EventLog, EventLogBackend, InMemoryEventLog};
pub use kernel::{
    SessionKernel, SessionKvLeaseFactory, provision_lease_store, provision_snapshot_store,
    provision_usage_store,
};
pub use lease::{
    SessionLeaseFactory, SessionLeaseGuard, SessionLeaseManager, SessionMutatingOperation, SessionKvLease,
    lease_bucket_config,
};

#[cfg(any(test, feature = "test-support"))]
pub use lease::{MockSessionLease, MockSessionLeaseFactory};
pub use materialize::materialize_from_events;
pub use recovery::{EventLogReader, RecoveredSession, SnapshotReader, recover_session};
pub use snapshot::SnapshotStore;
pub use state::{LeaseState, RecoveryState, SessionBusyPolicy, SessionOperationState};
pub use usage::UsageStore;
