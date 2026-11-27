use std::path::Path;

use tempfile::TempDir;

use crate::VolumeSyncPlan;

/// A workspace laid out in a scratch directory that is removed when this is dropped.
#[derive(Debug)]
pub struct MaterializedWorkspace {
    pub(crate) scratch: TempDir,
    pub(crate) volume_sync: Option<VolumeSyncPlan>,
}

impl MaterializedWorkspace {
    pub fn local_path(&self) -> &Path {
        self.scratch.path()
    }

    /// Where the workspace changes go back to, when the workspace is a writable volume.
    pub fn volume_sync(&self) -> Option<&VolumeSyncPlan> {
        self.volume_sync.as_ref()
    }
}
