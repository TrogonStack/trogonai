use tracing::debug;
use trogon_sandbox_contract::{FileContentHashing, FileRecursion};
use trogon_sandbox_file_ops::{ManifestSource, Tracking};

use super::constants::{BASELINE_PATH, BASELINE_TIMEOUT, BASELINE_USER};
use super::envd_source::EnvdSource;
use crate::envd::{Envd, EnvdTarget};

/// Records the default user's home as the lease-start baseline. Best effort: a sandbox that cannot be scanned in
/// time, or holds too many entries, keeps no baseline and `changes` then asks the caller for a checkpoint. A failed
/// scan never fails the lease.
pub async fn record_start_baseline(tracking: &Tracking, lease: &str, envd: &Envd, target: &EnvdTarget) {
    let mut source = EnvdSource::new(envd, target, BASELINE_USER);
    source.timeout = BASELINE_TIMEOUT;
    match source
        .manifest(BASELINE_PATH, FileRecursion::Recursive, FileContentHashing::Metadata)
        .await
    {
        Ok(manifest) => tracking.set_start_baseline(lease, BASELINE_PATH, manifest),
        Err(error) => {
            debug!(lease_id = %lease, error = %error, "no start baseline recorded");
            tracking.forget(lease);
        }
    }
}
