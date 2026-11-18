//! The timing rules of the browser SSE transport.

use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hasher},
    time::Duration,
};

mod constants;

use crate::js_string_number::js_string_to_number;

use constants::{BACKOFF_BASE_MS, BACKOFF_CAP_MS, JITTER_US};
pub(crate) use constants::{CONNECT_TIMEOUT, LIVENESS_TIMEOUT, POST_TIMEOUT, RECONNECT_BUDGET};

fn backoff_ms(attempt: u32) -> u64 {
    let doublings = 1u64.checked_shl(attempt.saturating_sub(1)).unwrap_or(u64::MAX);
    BACKOFF_BASE_MS.saturating_mul(doublings).min(BACKOFF_CAP_MS)
}

/// TypeScript `Bu`: the wait before reconnect number `attempt` (1 for the first), without jitter.
pub(crate) fn reconnect_delay(attempt: u32) -> Duration {
    Duration::from_millis(backoff_ms(attempt))
}

/// A random wait of up to 250 ms, TypeScript `Math.random() * 250`.
pub(crate) fn jitter() -> Duration {
    let sample = RandomState::new().build_hasher().finish();
    Duration::from_micros(sample % JITTER_US)
}

/// The wait before retrying a failed write for the `attempt`th time in a row: the server's `Retry-After`
/// (milliseconds) clamped to 1 to 30 seconds when it is finite, else the exponential backoff, plus `jitter`.
pub(crate) fn retry_delay(attempt: u32, retry_after_ms: Option<f64>, jitter: Duration) -> Duration {
    let base = match retry_after_ms.filter(|ms| ms.is_finite()) {
        Some(ms) => Duration::from_secs_f64(ms.clamp(1_000.0, 30_000.0) / 1_000.0),
        None => Duration::from_millis(backoff_ms(attempt)),
    };
    base + jitter
}

/// The `Retry-After` header as milliseconds, read the way the TypeScript transport does: `Number(header) * 1000`
/// when the header is not empty. The result can be infinite or NaN.
pub(crate) fn retry_after_ms(header: Option<&str>) -> Option<f64> {
    let text = header.filter(|text| !text.is_empty())?;
    Some(js_string_to_number(text) * 1_000.0)
}

#[cfg(test)]
#[path = "sse_retry_tests.rs"]
mod tests;
