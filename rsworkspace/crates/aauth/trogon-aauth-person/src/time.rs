//! Re-exports the pluggable clock used across the AAuth crates so callers of
//! `trogon-aauth-person` do not need a direct `trogon-aauth-verify`
//! dependency just to supply a clock.

pub use trogon_aauth_verify::TimeSource;

/// System-clock [`TimeSource`], for production use.
pub use trogon_aauth_verify::SystemTimeSource;
