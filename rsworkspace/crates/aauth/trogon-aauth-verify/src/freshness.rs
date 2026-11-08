//! Freshness of a signed request (ADR#0051 Decision 2): how long a signature
//! stays valid after it is created, and how much clock difference the
//! verifier tolerates on either side of that window.

pub use crate::clock_tolerance::{ClockTolerance, ClockToleranceError};
#[cfg(test)]
use crate::constants::{MAX_CLOCK_SKEW_SECS, MAX_SIGNATURE_VALIDITY_SECS};
pub use crate::signature_freshness::{FreshnessError, SignatureFreshness};
pub use crate::signature_validity::{SignatureValidity, SignatureValidityError};
pub use crate::signature_window::SignatureWindow;

#[cfg(test)]
mod tests;
