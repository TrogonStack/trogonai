//! The timing rules of the browser SSE transport.

use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hasher},
    time::Duration,
};

mod constants;

use crate::session_prompt::js_trim;

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
    Some(js_number(text) * 1_000.0)
}

/// JavaScript `Number(text)`.
fn js_number(text: &str) -> f64 {
    let text = js_trim(text);
    if text.is_empty() {
        return 0.0;
    }
    let radix = match text.get(..2) {
        Some("0x" | "0X") => 16,
        Some("0o" | "0O") => 8,
        Some("0b" | "0B") => 2,
        _ => 0,
    };
    if radix != 0 {
        let digits = text.get(2..).unwrap_or_default();
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return f64::NAN;
        }
        return digits.chars().fold(0.0, |value, c| {
            value * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or_default())
        });
    }
    let (sign, unsigned) = match text.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, text.strip_prefix('+').unwrap_or(text)),
    };
    if unsigned == "Infinity" {
        return sign * f64::INFINITY;
    }
    if is_decimal_literal(unsigned) {
        unsigned.parse::<f64>().map_or(f64::NAN, |value| sign * value)
    } else {
        f64::NAN
    }
}

/// `digits [. digits] [exponent]` or `. digits [exponent]`.
fn is_decimal_literal(text: &str) -> bool {
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (text, None),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (mantissa, None),
    };
    let has_digit = !whole.is_empty() || fraction.is_some_and(|fraction| !fraction.is_empty());
    digits(whole)
        && fraction.is_none_or(digits)
        && has_digit
        && exponent.is_none_or(|exponent| {
            let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            !exponent.is_empty() && digits(exponent)
        })
}

#[cfg(test)]
#[path = "sse_retry_tests.rs"]
mod tests;
