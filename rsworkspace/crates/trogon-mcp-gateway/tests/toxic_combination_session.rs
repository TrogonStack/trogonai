//! Acceptance for `GCP_TODO.md §3.5`: the first tool call in a forbidden
//! combination is allowed; the call that completes the set is denied
//! with the configured reason.

use trogon_mcp_gateway::policy::toxic_combinations::{
    ToxicCombinationRule, ToxicCombinationsConfig, ToxicCombinationsPolicy,
};

fn exfil_policy() -> ToxicCombinationsPolicy {
    ToxicCombinationsPolicy::new(ToxicCombinationsConfig {
        rules: vec![ToxicCombinationRule {
            tools: vec!["read_secrets".into(), "send_email".into()],
            reason: "exfiltration risk".into(),
        }],
    })
}

#[test]
fn read_secrets_then_send_email_denies_with_reason() {
    let policy = exfil_policy();
    assert!(
        policy.record_and_check("session-x", "read_secrets").is_none(),
        "first call must be allowed",
    );
    let deny = policy
        .record_and_check("session-x", "send_email")
        .expect("completing call must deny");
    assert_eq!(deny.reason, "exfiltration risk");
    assert!(deny.matched_tools.iter().any(|t| t == "read_secrets"));
    assert!(deny.matched_tools.iter().any(|t| t == "send_email"));
}

#[test]
fn other_sessions_are_unaffected() {
    let policy = exfil_policy();
    assert!(policy.record_and_check("session-x", "read_secrets").is_none());
    assert!(
        policy.record_and_check("session-y", "send_email").is_none(),
        "different session does not inherit the partial set",
    );
}
