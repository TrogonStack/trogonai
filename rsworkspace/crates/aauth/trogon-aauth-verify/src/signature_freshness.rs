use crate::clock_tolerance::ClockTolerance;
use crate::signature_validity::SignatureValidity;
use crate::signature_window::SignatureWindow;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FreshnessError {
    #[error("signature window does not end after it starts, or outlives the accepted validity")]
    InvalidWindow,
    #[error("signature is created further ahead than the tolerated clock skew")]
    NotYetValid,
    #[error("signature expired beyond the tolerated clock skew")]
    Expired,
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
            .expires()
            .checked_sub(window.created())
            .ok_or(FreshnessError::InvalidWindow)?;
        if span <= 0 || span > i64::from(self.validity.secs()) {
            return Err(FreshnessError::InvalidWindow);
        }
        let skew = i64::from(self.skew.secs());
        if window.created() > now.saturating_add(skew) {
            return Err(FreshnessError::NotYetValid);
        }
        if now > window.expires().saturating_add(skew) {
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
