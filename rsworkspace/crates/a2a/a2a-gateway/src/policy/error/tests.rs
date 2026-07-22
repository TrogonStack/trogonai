use std::error::Error as _;

use a2a_redaction::RedactionError;

use super::*;

#[test]
fn tier2_eval_error_execution_variant_renders_message() {
    let err = Tier2EvalError::execution("interpreter blew up");
    assert_eq!(format!("{err}"), "CEL execution failed: interpreter blew up");
    assert!(matches!(err, Tier2EvalError::Execution { .. }));
}

#[test]
fn tier2_eval_error_non_bool_result_variant_renders_value_type() {
    let err = Tier2EvalError::non_bool_result("Int(42)");
    assert_eq!(format!("{err}"), "CEL rule must return bool, got Int(42)");
    assert!(matches!(err, Tier2EvalError::NonBoolResult { .. }));
}

#[test]
fn tier2_eval_error_binding_variant_carries_stage() {
    let err = Tier2EvalError::binding("request", "serialize failed");
    assert_eq!(format!("{err}"), "CEL binding `request` failed: serialize failed");
    assert!(matches!(err, Tier2EvalError::Binding { .. }));
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
    let inner = Tier2EvalError::execution("cel error");
    let inner_display = inner.to_string();
    let err: PolicyError = inner.into();
    assert!(matches!(err, PolicyError::Tier2(_)));
    assert_eq!(format!("{err}"), "tier2 evaluation failed");
    assert_eq!(err.source().map(|s| s.to_string()), Some(inner_display));
}
