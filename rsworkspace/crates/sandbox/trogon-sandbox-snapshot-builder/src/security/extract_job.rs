use serde::{Deserialize, Serialize};

use super::ScanLimits;

/// What the isolated extractor is told through its job file.
#[derive(Serialize, Deserialize)]
pub(super) struct ExtractJob {
    pub(super) limits: ScanLimits,
    pub(super) gzip: bool,
}
