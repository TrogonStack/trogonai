//! Limits and bounds shared by the value objects of this crate.

use std::time::Duration;

/// Longest accepted agent, node, model, or hold identifier, in bytes.
pub const MAX_IDENTIFIER_BYTES: usize = 256;

/// Longest accepted request-rule name, in bytes.
pub const MAX_RULE_NAME_BYTES: usize = 256;

/// Longest accepted host name, in bytes (RFC 1035 section 2.3.4).
pub const MAX_HOST_BYTES: usize = 253;

/// Longest accepted label of a host name, in bytes (RFC 1035 section 2.3.4).
pub const MAX_HOST_LABEL_BYTES: usize = 63;

/// The cache TTL of ADR#0049, the backstop when the event path fails entirely.
pub const REVOCATION_CACHE_TTL_SECS: u64 = 300;

/// The per-key jitter ADR#0049 adds on top of the cache TTL.
pub const REVOCATION_CACHE_JITTER_SECS: u64 = 30;

/// The hard ceiling of ADR#0071 Decision 7 on how stale a projection may be:
/// the cache TTL plus the jitter. No enforcement point may configure a budget
/// above it.
pub const MAX_STALENESS_BUDGET: Duration =
    Duration::from_secs(REVOCATION_CACHE_TTL_SECS + REVOCATION_CACHE_JITTER_SECS);
