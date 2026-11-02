//! Freshness of a signed request (ADR#0051 Decision 2): how long a signature
//! stays valid after it is created, and how much clock difference the
//! verifier tolerates on either side of that window.

use crate::constants::{
    DEFAULT_CLOCK_SKEW_SECS, DEFAULT_SIGNATURE_VALIDITY_SECS, MAX_CLOCK_SKEW_SECS, MAX_SIGNATURE_VALIDITY_SECS,
};

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

/// Clock difference a verifier tolerates between its own clock and the
/// signer's, applied as clock skew to a signature's window and as expiry
/// leeway to the tokens it presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockTolerance(u32);

impl ClockTolerance {
    /// # Errors
    ///
    /// [`ClockToleranceError`] when `secs` exceeds [`MAX_CLOCK_SKEW_SECS`].
    pub fn new(secs: u32) -> Result<Self, ClockToleranceError> {
        if secs > MAX_CLOCK_SKEW_SECS {
            return Err(ClockToleranceError { secs });
        }
        Ok(Self(secs))
    }

    #[must_use]
    pub fn secs(self) -> u32 {
        self.0
    }
}

impl Default for ClockTolerance {
    fn default() -> Self {
        Self(DEFAULT_CLOCK_SKEW_SECS)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("clock tolerance must be at most {max}s, got {secs}s", max = MAX_CLOCK_SKEW_SECS)]
pub struct ClockToleranceError {
    secs: u32,
}

/// The `created` and `expires` signature parameters, in seconds since the
/// epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignatureWindow {
    created: i64,
    expires: i64,
}

impl SignatureWindow {
    #[must_use]
    pub fn new(created: i64, expires: i64) -> Self {
        Self { created, expires }
    }

    #[must_use]
    pub fn created(&self) -> i64 {
        self.created
    }

    #[must_use]
    pub fn expires(&self) -> i64 {
        self.expires
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SignatureFreshness {
    validity: SignatureValidity,
    skew: ClockTolerance,
}

impl SignatureFreshness {
    #[must_use]
    pub fn new(validity: SignatureValidity, skew: ClockTolerance) -> Self {
        Self { validity, skew }
    }

    #[must_use]
    pub fn validity(&self) -> SignatureValidity {
        self.validity
    }

    #[must_use]
    pub fn skew(&self) -> ClockTolerance {
        self.skew
    }

    /// # Errors
    ///
    /// [`FreshnessError::InvalidWindow`] when `expires` is not after `created`
    /// or the window outlives [`Self::validity`],
    /// [`FreshnessError::NotYetValid`] when `created` is further ahead of
    /// `now` than the tolerated skew, and [`FreshnessError::Expired`] when
    /// `now` is past `expires` by more than the tolerated skew.
    pub fn check(&self, window: SignatureWindow, now: i64) -> Result<(), FreshnessError> {
        let span = window
            .expires
            .checked_sub(window.created)
            .ok_or(FreshnessError::InvalidWindow)?;
        if span <= 0 || span > i64::from(self.validity.secs()) {
            return Err(FreshnessError::InvalidWindow);
        }
        let skew = i64::from(self.skew.secs());
        if window.created > now.saturating_add(skew) {
            return Err(FreshnessError::NotYetValid);
        }
        if now > window.expires.saturating_add(skew) {
            return Err(FreshnessError::Expired);
        }
        Ok(())
    }

    /// How long a replay record has to live: a signature admitted at
    /// `created - skew` stays admissible until `expires + skew`
    /// (ADR#0051 Decision 4).
    #[must_use]
    pub fn record_ttl_secs(&self) -> u32 {
        self.validity.secs() + 2 * self.skew.secs()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FreshnessError {
    #[error("signature window does not end after it starts, or outlives the accepted validity")]
    InvalidWindow,
    #[error("signature is created further ahead than the tolerated clock skew")]
    NotYetValid,
    #[error("signature expired beyond the tolerated clock skew")]
    Expired,
}

#[cfg(test)]
mod tests;
