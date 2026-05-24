//! Host-side refusal convention for Tier-3 `redact_part` guest output.

/// UTF-8 prefix written to guest linear memory when a skill refuses to redact a part.
///
/// Collisions with legitimate JSON payloads are bundle bugs; the gateway logs a warning.
pub const TIER3_REFUSE_SENTINEL: &[u8] = b"A2A_T3_REFUSE";

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
mod tests {
    use super::*;

    #[test]
    fn detects_refusal_sentinel_prefix() {
        assert!(output_is_tier3_refusal(b"A2A_T3_REFUSE"));
        assert!(output_is_tier3_refusal(b"A2A_T3_REFUSE:SkillPolicyDeniedPart"));
        assert!(!output_is_tier3_refusal(br#"{"ok":true}"#));
    }

    #[test]
    fn parses_reason_tag_after_colon() {
        assert_eq!(
            tier3_refusal_reason_tag(b"A2A_T3_REFUSE:UnauthorizedDataCategory"),
            Some("UnauthorizedDataCategory")
        );
        assert_eq!(tier3_refusal_reason_tag(b"A2A_T3_REFUSE"), None);
    }
}
