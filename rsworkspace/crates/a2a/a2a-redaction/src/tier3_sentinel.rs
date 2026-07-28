//! Host-side refusal convention for Tier-3 `redact_part` guest output.

use crate::constants::TIER3_REFUSE_SENTINEL;

pub fn output_is_tier3_refusal(out: &[u8]) -> bool {
    out.starts_with(TIER3_REFUSE_SENTINEL)
}

/// Optional reason tag after `A2A_T3_REFUSE:` (e.g. `UnauthorizedDataCategory`).
pub fn tier3_refusal_reason_tag(out: &[u8]) -> Option<&str> {
    const PREFIX: &[u8] = b"A2A_T3_REFUSE:";
    if !out.starts_with(PREFIX) {
        return None;
    }
    std::str::from_utf8(&out[PREFIX.len()..])
        .ok()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests;
