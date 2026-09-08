//! Crate-wide constants.

/// Pinned ARD manifest spec version for this crate snapshot.
pub const SPEC_VERSION: &str = "1.0";

/// Pinned ARD ai-catalog JSON Schema.
///
/// Source: `ards-project/ard-spec` commit
/// `832347bda6af4ce3b61bd250c14a8e899d3ff942`.
pub const AI_CATALOG_JSON_SCHEMA: &str = include_str!("../schemas/ai-catalog.schema.json");

pub(crate) const URN_AIR_PREFIX: &str = "urn:air:";

pub(crate) const MIN_REPRESENTATIVE_QUERIES: usize = 2;
pub(crate) const MAX_REPRESENTATIVE_QUERIES: usize = 5;
