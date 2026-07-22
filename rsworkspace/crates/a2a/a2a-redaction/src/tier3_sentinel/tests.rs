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
