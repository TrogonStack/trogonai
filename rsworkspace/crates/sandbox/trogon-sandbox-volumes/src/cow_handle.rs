use std::path::PathBuf;

use crate::VolumePrefix;

/// A materialized copy-on-write base and the directory collecting this lease's changes.
#[derive(Debug, Clone)]
pub struct CowHandle {
    pub base_prefix: VolumePrefix,
    pub lease_prefix: VolumePrefix,
    pub delta_dir: PathBuf,
}
