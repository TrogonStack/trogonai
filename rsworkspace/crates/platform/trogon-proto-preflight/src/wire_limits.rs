#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireLimits {
    max_bytes: usize,
    max_depth: usize,
    max_fields: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WireLimitsError {
    #[error("wire byte limit must be between one and sixteen mebibytes")]
    Bytes,
    #[error("wire nesting limit must be between one and sixty-four")]
    Depth,
    #[error("wire field limit must be between one and 65536")]
    Fields,
}

impl WireLimits {
    pub fn new(max_bytes: usize, max_depth: usize, max_fields: usize) -> Result<Self, WireLimitsError> {
        if !(1..=16 * 1024 * 1024).contains(&max_bytes) {
            return Err(WireLimitsError::Bytes);
        }
        if !(1..=64).contains(&max_depth) {
            return Err(WireLimitsError::Depth);
        }
        if !(1..=65536).contains(&max_fields) {
            return Err(WireLimitsError::Fields);
        }
        Ok(Self {
            max_bytes,
            max_depth,
            max_fields,
        })
    }

    pub fn max_bytes(self) -> usize {
        self.max_bytes
    }
    pub fn max_depth(self) -> usize {
        self.max_depth
    }
    pub fn max_fields(self) -> usize {
        self.max_fields
    }
}
