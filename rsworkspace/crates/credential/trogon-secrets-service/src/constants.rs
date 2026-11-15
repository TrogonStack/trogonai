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

/// Why a credential state read that found its bound gone answers nothing.
pub(crate) const BOUND_VANISHED: &str = "the last event of the credential's subject was removed during the read";
