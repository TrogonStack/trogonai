//! Claims-required round trip per "Claims Required" (#requirement-claims)
//! and "AS Response" (#as-token-endpoint): when the AS returns
//! `202 Accepted` / `requirement=claims`, the PS POSTs a
//! [`ClaimsSubmission`] to the `Location` URL and the AS resumes evaluation
//! of the original request with those claims added.
//!
//! This module holds the minimal state needed to resume: the verified
//! request plus the PS issuer it came from. It intentionally does not model
//! `202`'s other requirement kinds (`interaction`, `approval`, `402`
//! payment) -- those extend trust itself (#ps-as-trust-establishment) rather
//! than feeding a policy decision, and are out of scope for this crate's
//! first slice (see `lib.rs` deviations).

use std::collections::HashMap;
use std::sync::Mutex;

use trogon_identity_types::aauth::federation::ClaimsSubmission;

use crate::trust::TrustedIssuer;
use crate::verify::VerifiedRequest;

/// Opaque identifier for a pending claims-required request, embedded in the
/// `Location` URL the AS returns on `202`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PendingRequestId(String);

impl PendingRequestId {
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One request awaiting a [`ClaimsSubmission`] from the PS.
pub struct PendingRequest {
    pub verified: VerifiedRequest,
    pub trust: TrustedIssuer,
}

/// In-memory store of requests parked on `requirement=claims`. Keyed by
/// [`PendingRequestId`]; `AccessServer` owns one instance per deployment.
#[derive(Default)]
pub struct PendingRequestStore {
    entries: Mutex<HashMap<PendingRequestId, PendingRequest>>,
}

impl PendingRequestStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn insert(&self, id: PendingRequestId, entry: PendingRequest) {
        #[allow(clippy::unwrap_used)]
        // A poisoned mutex means a prior holder panicked mid-critical-section
        // elsewhere in this process; there is no safe recovery for pending
        // AS state at that point, so propagate the panic rather than
        // silently dropping the pending request.
        let mut guard = self.entries.lock().unwrap();
        guard.insert(id, entry);
    }

    /// Remove and return the pending request. Consumes it so a
    /// [`ClaimsSubmission`] can only be applied once per `Location` URL.
    #[must_use]
    pub fn take(&self, id: &PendingRequestId) -> Option<PendingRequest> {
        #[allow(clippy::unwrap_used)]
        let mut guard = self.entries.lock().unwrap();
        guard.remove(id)
    }
}

/// Body accepted at the claims-resume endpoint, re-exported here so callers
/// of [`crate::server::AccessServer::resume_with_claims`] don't need a
/// separate import from `trogon-identity-types`.
pub type ResumeBody = ClaimsSubmission;

#[cfg(test)]
mod tests;
