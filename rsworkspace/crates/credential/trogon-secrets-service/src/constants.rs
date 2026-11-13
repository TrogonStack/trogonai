use std::time::Duration;

/// The longest a released value may be held, per the ADR#0049 staleness bound.
///
/// A policy may shorten it and never lengthen it, so a revoked credential
/// leaves every holder within this window.
pub const RELEASE_TTL_BOUND: Duration = Duration::from_secs(300);

/// The generation a single released value reports.
///
/// Only a bundle of several values has generations to commit; a release of one
/// value has none, which the wire contract spells as zero.
pub const UNBUNDLED_GENERATION: u64 = 0;

/// The destination a release record names when `host:port` is longer than an
/// audit name may be (a 250 byte host and a port do not fit). It is the egress
/// proxy's spelling, so both records of the dual audit (ADR#0023 Decision 6)
/// name such a destination alike. The release is still recorded; only the
/// spelling of where is replaced.
pub(crate) const DESTINATION_TOO_LONG_NAME: &str = "destination_too_long";
