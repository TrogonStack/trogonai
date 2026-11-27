//! The count of frames the TAP ingress guard dropped, exported through the worker's
//! metrics registry.
//!
//! The counter lives in the lease's network namespace and disappears with it, so each lease
//! adds its final count to the process-wide total just before its namespace is deleted.
//! A lease that is still running has not been counted yet.

use std::sync::OnceLock;

use prometheus::{IntCounter, Registry};

static SPOOFED_FRAMES: OnceLock<IntCounter> = OnceLock::new();

/// Registers the counter in `registry`. Later calls keep the first counter.
pub fn register(registry: &Registry) -> anyhow::Result<()> {
    let counter = IntCounter::new(
        "sandbox_firecracker_spoofed_frames_total",
        "Frames dropped on a guest TAP for a forged source MAC or a VLAN tag, counted when a lease's network is torn down",
    )?;
    registry.register(Box::new(counter.clone()))?;
    let _ = SPOOFED_FRAMES.set(counter);
    Ok(())
}

/// Adds `frames` to the total. Does nothing before [`register`].
pub fn record(frames: u64) {
    if frames > 0
        && let Some(counter) = SPOOFED_FRAMES.get()
    {
        counter.inc_by(frames);
    }
}

#[cfg(test)]
mod tests;
