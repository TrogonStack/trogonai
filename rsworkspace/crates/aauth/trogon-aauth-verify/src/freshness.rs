//! Freshness of a signed request (ADR#0051 Decision 2): how long a signature
//! stays valid after it is created, and how much clock difference the
//! verifier tolerates on either side of that window.

pub use crate::clock_tolerance::{ClockTolerance, ClockToleranceError};
#[cfg(test)]
use crate::constants::{MAX_CLOCK_SKEW_SECS, MAX_SIGNATURE_VALIDITY_SECS};
pub use crate::signature_freshness::SignatureFreshness;
pub use crate::signature_validity::{SignatureValidity, SignatureValidityError};
pub use crate::signature_window::SignatureWindow;

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
