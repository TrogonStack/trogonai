use crate::constants::{DEFAULT_SIGNATURE_VALIDITY_SECS, MAX_SIGNATURE_VALIDITY_SECS};

/// Longest span a verifier accepts between a signature's `created` and
/// `expires`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignatureValidity(u32);

impl SignatureValidity {
    /// # Errors
    ///
    /// [`SignatureValidityError`] when `secs` is zero or exceeds
    /// [`MAX_SIGNATURE_VALIDITY_SECS`].
    pub fn new(secs: u32) -> Result<Self, SignatureValidityError> {
        if secs == 0 || secs > MAX_SIGNATURE_VALIDITY_SECS {
            return Err(SignatureValidityError { secs });
        }
        Ok(Self(secs))
    }

    #[must_use]
    pub fn secs(self) -> u32 {
        self.0
    }
}

impl Default for SignatureValidity {
    fn default() -> Self {
        Self(DEFAULT_SIGNATURE_VALIDITY_SECS)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("signature validity must be between 1 and {max}s, got {secs}s", max = MAX_SIGNATURE_VALIDITY_SECS)]
pub struct SignatureValidityError {
    secs: u32,
}
