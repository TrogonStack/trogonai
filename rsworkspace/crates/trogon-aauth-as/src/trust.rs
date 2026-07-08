//! PS-AS trust registry per draft "PS-AS Federation" / "PS-AS Trust
//! Establishment" (#ps-as-federation): the AS only accepts token requests
//! signed by a PS it recognizes -- pre-established, bound via prior
//! interaction, billed, or trusted for claims-only decisions. This module
//! models the registry of recognized PS issuers; the interaction/payment/
//! claims *establishment* flows themselves are carried by [`crate::policy`]
//! and [`crate::pending`].

use std::collections::HashMap;

/// Trust basis for a federated PS, per "PS-AS Trust Establishment". Kept
/// distinct from [`crate::policy::Decision`] because trust is a durable,
/// per-PS fact the registry owns, while a `Decision` is the per-request
/// outcome the policy computes using that fact as an input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustBasis {
    /// "Pre-established": a business relationship configured out of band.
    PreEstablished,
    /// "Interaction": trust established after a one-time user binding at the AS.
    Interaction,
    /// "Payment": trust established after a billing relationship was settled.
    Payment,
    /// "Claims only": the AS trusts any PS that can supply sufficient
    /// identity claims, without a prior relationship.
    ClaimsOnly,
}

/// A federated PS issuer the AS recognizes, and the basis on which it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustedIssuer {
    iss: PsIssuer,
    basis: TrustBasisRecord,
}

impl TrustedIssuer {
    #[must_use]
    pub fn new(iss: PsIssuer, basis: TrustBasisRecord) -> Self {
        Self { iss, basis }
    }

    #[must_use]
    pub fn issuer(&self) -> &PsIssuer {
        &self.iss
    }

    #[must_use]
    pub fn basis(&self) -> &TrustBasisRecord {
        &self.basis
    }
}

/// [`TrustBasis`] plus any basis-specific detail the policy layer needs.
/// Modeled as a value object (not a bare enum) so future bases (e.g. payment
/// plan tier) can carry data without widening every match arm across the
/// crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustBasisRecord {
    PreEstablished,
    Interaction,
    Payment,
    ClaimsOnly,
}

impl TrustBasisRecord {
    #[must_use]
    pub fn kind(&self) -> TrustBasis {
        match self {
            TrustBasisRecord::PreEstablished => TrustBasis::PreEstablished,
            TrustBasisRecord::Interaction => TrustBasis::Interaction,
            TrustBasisRecord::Payment => TrustBasis::Payment,
            TrustBasisRecord::ClaimsOnly => TrustBasis::ClaimsOnly,
        }
    }
}

/// Validated PS issuer identifier: a non-empty HTTPS URL, per "Server
/// Identifiers". Rejecting malformed entries at construction time is how the
/// registry "fails loudly on invalid entries" rather than silently admitting
/// an issuer no token will ever match.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PsIssuer(String);

impl PsIssuer {
    pub fn new(raw: impl Into<String>) -> Result<Self, TrustRegistryError> {
        let value = raw.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(TrustRegistryError::EmptyIssuer);
        }
        if !trimmed.starts_with("https://") {
            return Err(TrustRegistryError::NotHttps(trimmed.to_string()));
        }
        Ok(Self(trimmed.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Registry of PS issuers the AS federates with. Built once at startup from
/// typed configuration; construction fails loudly (returns `Err`) rather than
/// silently dropping malformed entries, so a misconfigured deployment cannot
/// come up believing it trusts a PS it does not.
#[derive(Clone, Debug, Default)]
pub struct TrustRegistry {
    issuers: HashMap<String, TrustedIssuer>,
}

impl TrustRegistry {
    /// Build a registry from configuration entries. Fails on the first
    /// invalid issuer or duplicate entry -- see module docs.
    pub fn from_entries(
        entries: impl IntoIterator<Item = (String, TrustBasisRecord)>,
    ) -> Result<Self, TrustRegistryError> {
        let mut issuers = HashMap::new();
        for (raw_iss, basis) in entries {
            let iss = PsIssuer::new(raw_iss)?;
            let key = iss.as_str().to_string();
            if issuers.contains_key(&key) {
                return Err(TrustRegistryError::DuplicateIssuer(key));
            }
            issuers.insert(key, TrustedIssuer::new(iss, basis));
        }
        Ok(Self { issuers })
    }

    #[must_use]
    pub fn empty() -> Self {
        Self {
            issuers: HashMap::new(),
        }
    }

    /// Register or replace a single trusted PS, e.g. after dynamic trust
    /// establishment (#ps-as-trust-establishment) completes at runtime.
    pub fn trust(&mut self, iss: PsIssuer, basis: TrustBasisRecord) {
        self.issuers
            .insert(iss.as_str().to_string(), TrustedIssuer::new(iss, basis));
    }

    /// Look up a PS issuer. `Err(UnknownIssuer)` is the "reject unknown PS"
    /// path required by the AS token endpoint.
    pub fn require(&self, iss: &str) -> Result<&TrustedIssuer, TrustRegistryError> {
        self.issuers
            .get(iss)
            .ok_or_else(|| TrustRegistryError::UnknownIssuer(iss.to_string()))
    }

    #[must_use]
    pub fn contains(&self, iss: &str) -> bool {
        self.issuers.contains_key(iss)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrustRegistryError {
    #[error("PS issuer must not be empty")]
    EmptyIssuer,
    #[error("PS issuer must be an https URL, got {0:?}")]
    NotHttps(String),
    #[error("duplicate PS issuer in trust registry: {0}")]
    DuplicateIssuer(String),
    #[error("unknown or untrusted PS issuer: {0}")]
    UnknownIssuer(String),
}

#[cfg(test)]
mod tests;
