use std::path::PathBuf;

use super::ScanLimits;

/// How a Firecracker snapshot is turned into a scannable tree.
#[derive(Debug, Clone, Default)]
pub struct ScanPreparation {
    pub limits: ScanLimits,
    /// The program that runs the isolated extraction. Unset uses the running executable.
    pub extractor_program: Option<PathBuf>,
}
