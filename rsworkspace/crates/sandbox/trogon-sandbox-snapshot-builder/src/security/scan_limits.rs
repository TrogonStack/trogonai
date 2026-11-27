use serde::{Deserialize, Serialize};

use super::ScanLimitsError;
use crate::constants::{
    EXTRACT_TIMEOUT_MS, MAX_COMPRESSED_BYTES, MAX_DEPTH, MAX_ENTRIES, MAX_EXTRACTED_BYTES, MAX_FILE_BYTES,
    MAX_IMAGE_BYTES,
};

/// The byte, entry, depth, and time budgets an extraction runs under. Every field must be positive and no larger
/// than its default.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanLimits {
    pub max_compressed_bytes: u64,
    pub max_image_bytes: u64,
    pub max_extracted_bytes: u64,
    pub max_file_bytes: u64,
    pub max_entries: usize,
    pub max_depth: usize,
    pub timeout_ms: u64,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_compressed_bytes: MAX_COMPRESSED_BYTES,
            max_image_bytes: MAX_IMAGE_BYTES,
            max_extracted_bytes: MAX_EXTRACTED_BYTES,
            max_file_bytes: MAX_FILE_BYTES,
            max_entries: MAX_ENTRIES,
            max_depth: MAX_DEPTH,
            timeout_ms: EXTRACT_TIMEOUT_MS,
        }
    }
}

impl ScanLimits {
    pub fn validate(&self) -> Result<(), ScanLimitsError> {
        let maximum = Self::default();
        let within = |value: u64, ceiling: u64| value > 0 && value <= ceiling;
        if within(self.max_compressed_bytes, maximum.max_compressed_bytes)
            && within(self.max_image_bytes, maximum.max_image_bytes)
            && within(self.max_extracted_bytes, maximum.max_extracted_bytes)
            && within(self.max_file_bytes, maximum.max_file_bytes)
            && within(self.max_entries as u64, maximum.max_entries as u64)
            && within(self.max_depth as u64, maximum.max_depth as u64)
            && within(self.timeout_ms, maximum.timeout_ms)
        {
            Ok(())
        } else {
            Err(ScanLimitsError::Invalid)
        }
    }
}
