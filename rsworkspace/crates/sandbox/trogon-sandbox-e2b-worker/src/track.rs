//! Change tracking over envd.
//!
//! E2B has no diff API, so the worker fingerprints a tree itself: a `find | stat` script run through envd yields a
//! [`trogon_sandbox_file_ops::Manifest`] (kind, size, nanosecond mtime, optionally a sha256), and the shared
//! [`trogon_sandbox_file_ops::Tracking`] state machine diffs two of them. At lease start the worker records the
//! default user's home as the baseline `changes` uses when no checkpoint is named; checkpoints are taken on request.
//! Baselines and checkpoints live in worker memory only, so a worker restart loses them and `changes` then answers
//! that no baseline is held. Pausing a sandbox keeps its filesystem, so they stay valid across suspend and wake.
//!
//! Bounds: `FileOp::TRACKING_MAX_ENTRIES` entries and [`MAX_MANIFEST_BYTES`] of script output per manifest (enforced
//! inside the sandbox, so the reply envd sends back is bounded too), [`BASELINE_TIMEOUT`] for the lease-start scan. A
//! tree past either bound is refused with `TooLarge`; nothing is truncated silently.
//!
//! This behaviour is verified against an in-process fake envd only, never a live sandbox.

mod constants;
mod envd_source;
mod record_start_baseline;
#[cfg(test)]
mod tests;

pub use constants::{BASELINE_PATH, BASELINE_TIMEOUT, MAX_MANIFEST_BYTES};
pub use envd_source::EnvdSource;
pub use record_start_baseline::record_start_baseline;
