use crate::constants::{DEFAULT_SERVER_NONCE_TTL_SECS, MAX_SERVER_NONCE_TTL_SECS};

/// How long a server-issued nonce stays usable after it is issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonceTtl(u32);

impl NonceTtl {
    /// # Errors
    ///
    /// [`NonceTtlError`] when `secs` is zero or exceeds
    /// [`MAX_SERVER_NONCE_TTL_SECS`].
    pub fn new(secs: u32) -> Result<Self, NonceTtlError> {
        if secs == 0 || secs > MAX_SERVER_NONCE_TTL_SECS {
            return Err(NonceTtlError { secs });
        }
        Ok(Self(secs))
    }

    #[must_use]
    pub fn secs(self) -> u32 {
        self.0
    }
}

impl Default for NonceTtl {
    fn default() -> Self {
        Self(DEFAULT_SERVER_NONCE_TTL_SECS)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("nonce lifetime must be between 1 and {max}s, got {secs}s", max = MAX_SERVER_NONCE_TTL_SECS)]
pub struct NonceTtlError {
    secs: u32,
}
