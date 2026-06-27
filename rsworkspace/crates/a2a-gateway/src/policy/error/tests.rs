use std::error::Error as _;

use a2a_redaction::RedactionError;

use super::*;

#[test]
fn tier2_eval_error_round_trips_message_through_display() {
    let err = Tier2EvalError::new("rule rejected request");
    assert_eq!(format!("{err}"), "rule rejected request");
    assert_eq!(err.as_str(), "rule rejected request");
}

#[test]
fn policy_error_redaction_variant_carries_source_chain() {
    // RedactionError must thread through `Error::source` so the audit
    // subject can serialize root-cause separately from the variant tag.
    let inner = RedactionError::WasmEngine("missing engine".to_string());
    let inner_display = inner.to_string();
    let err: PolicyError = inner.into();
    assert!(matches!(err, PolicyError::Redaction(_)));
    assert_eq!(format!("{err}"), "redaction tier failed");
    assert_eq!(err.source().map(|s| s.to_string()), Some(inner_display));
}

#[test]
fn policy_error_tier2_variant_carries_source_chain() {
    let inner = Tier2EvalError::new("cel error");
    let err: PolicyError = inner.into();
    assert!(matches!(err, PolicyError::Tier2(_)));
    assert_eq!(format!("{err}"), "tier2 evaluation failed");
    assert_eq!(err.source().map(|s| s.to_string()), Some("cel error".to_string()));
}
