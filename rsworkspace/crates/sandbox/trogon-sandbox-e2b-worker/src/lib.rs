//! E2B provider worker. E2B sandboxes are Firecracker microVMs created through a REST control plane; commands and
//! files go to the `envd` daemon inside each sandbox.

// `LeaseError` is the wire denial every handler speaks; it is large by design and only built on a refusal.
#![allow(clippy::result_large_err)]

mod adapter;
mod api;
mod assignments;
mod bounded_retry;
mod constants;
mod custody_bucket;
mod custody_bucket_error;
mod envd;
mod exec;
mod file_ops;
mod heartbeat;
mod kv_outcome;
mod launch;
mod launch_error;
mod lease_cleanup;
mod lease_sandbox;
mod native_sandbox_id;
mod native_sandbox_id_error;
mod network;
mod profile;
mod publish_lifecycle;
mod snapshot_custody;
mod snapshot_lifecycle_error;
mod snapshots;
pub mod terminal;
mod track;
mod ulid;
mod ulid_error;
mod watch;
mod worker;
mod worker_args;

pub use adapter::{
    E2bAuthorityError, E2bDispatchError, E2bProviderError, E2bWorkerProvider, LeaseLifetimeError, NATIVE_OPTION_KEYS,
};
pub use api::{
    ApiError, DEFAULT_API_URL, DEFAULT_DOMAIN, E2bApi, E2bConfig, E2bConfigError, NativeAccessTokenError, NewSandbox,
    SandboxDetail, SandboxInfo,
};
pub use assignments::AssignmentsError;
pub use constants::{DEFAULT_HTTP_TIMEOUT_SECS, DEFAULT_MAX_CONCURRENT_LEASES, DEFAULT_NATS_URL, DEFAULT_TEMPLATE};
pub use custody_bucket_error::CustodyBucketError;
pub use envd::{Envd, EnvdTarget, EnvdTargetError};
pub use exec::{E2bExec, ExecError};
pub use heartbeat::build_heartbeat;
pub use launch::launch;
pub use launch_error::LaunchError;
pub use lease_cleanup::LeaseCleanupError;
pub use lease_sandbox::LeaseSandbox;
pub use native_sandbox_id_error::NativeSandboxIdError;
pub use network::{E2bNetwork, translate_network_policy};
pub use profile::PROFILE;
pub use snapshot_custody::{SnapshotClockError, SnapshotCustodyError};
pub use snapshot_lifecycle_error::SnapshotLifecycleError;
pub use snapshots::{SnapshotManifestError, SnapshotManifestsError, SnapshotReferenceError};
pub use track::{BASELINE_PATH, BASELINE_TIMEOUT, MAX_MANIFEST_BYTES};
pub use watch::{MAX_WATCHES_PER_LEASE, WATCH_BUFFER_EVENTS, WATCH_IDLE_EXPIRY, Watches};
pub use worker::{E2bWorkerError, run};
pub use worker_args::WorkerArgs;
