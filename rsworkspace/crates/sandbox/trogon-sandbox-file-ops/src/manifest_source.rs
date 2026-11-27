use std::time::Duration;

use trogon_sandbox_contract::{FileContentHashing, FileRecursion};

use crate::{ChangeHint, FileOpError, Manifest, ManifestDiff};

/// Where a worker's manifests come from.
#[allow(async_fn_in_trait)]
pub trait ManifestSource {
    /// Fingerprints `path`: the single entry for a file, or the entries under a directory (all
    /// depths when recursive, direct children otherwise). The directory itself is not an entry. A
    /// tree past [`trogon_sandbox_contract::FileOp::TRACKING_MAX_ENTRIES`] answers `TooLarge`; a
    /// missing path answers `NotFound`.
    async fn manifest(
        &self,
        path: &str,
        recursion: FileRecursion,
        hashing: FileContentHashing,
    ) -> Result<Manifest, FileOpError>;

    /// Changes since the lease start when the provider can report them without a stored baseline.
    /// Used when no lease-start baseline was recorded with
    /// [`Tracking::set_start_baseline`](crate::Tracking::set_start_baseline), and for the paths
    /// outside the baseline's tree when one was. `None` means the provider cannot say.
    async fn changes_since_start(&self) -> Option<Result<ManifestDiff, FileOpError>> {
        None
    }

    /// How often a parked watch poll rescans.
    fn scan_interval(&self) -> Duration {
        Duration::from_millis(250)
    }

    /// Blocks a parked watch poll until something under `path` may have changed or `timeout`
    /// passes, whichever is first; the poll rescans either way. The default sleeps one
    /// [`Self::scan_interval`]. A source that can block on change notifications returns as soon as
    /// one arrives.
    async fn wait_for_change(&self, _path: &str, _recursion: FileRecursion, timeout: Duration) {
        tokio::time::sleep(timeout.min(self.scan_interval())).await;
    }

    /// Like [`Self::wait_for_change`], for a source that keeps a change feed per watch. The
    /// default ignores the watch.
    async fn wait_for_watch(&self, _watch_id: &str, path: &str, recursion: FileRecursion, timeout: Duration) {
        self.wait_for_change(path, recursion, timeout).await;
    }

    /// What the watch's change feed reported since the last call, oldest first. Scans stay the
    /// source of truth; hints only add what two scans cannot tell apart. The default has no feed.
    fn drain_hints(&self, _watch_id: &str) -> Vec<ChangeHint> {
        Vec::new()
    }
}
