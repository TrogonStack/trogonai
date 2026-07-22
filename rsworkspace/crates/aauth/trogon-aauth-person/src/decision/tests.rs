use super::*;

#[test]
fn grant_decision_carries_scope() {
    let decision = PolicyDecision::Grant {
        scope: "calendar.read".to_string(),
    };
    assert_eq!(
        decision,
        PolicyDecision::Grant {
            scope: "calendar.read".to_string()
        }
    );
}

#[test]
fn deny_decision_carries_reason() {
    let decision = PolicyDecision::Deny {
        reason: "outside working hours".to_string(),
    };
    match decision {
        PolicyDecision::Deny { reason } => assert_eq!(reason, "outside working hours"),
        other => panic!("expected Deny, got {other:?}"),
    }
}

#[test]
fn needs_clarification_decision_carries_options() {
    let decision = PolicyDecision::NeedsClarification {
        clarification: "Why do you need write access?".to_string(),
        options: Some(vec!["Read-only".to_string(), "Full access".to_string()]),
    };
    match decision {
        PolicyDecision::NeedsClarification { clarification, options } => {
            assert_eq!(clarification, "Why do you need write access?");
            assert_eq!(options.unwrap().len(), 2);
        }
        other => panic!("expected NeedsClarification, got {other:?}"),
    }
}

#[test]
fn decision_variants_are_distinguishable() {
    assert_ne!(PolicyDecision::NeedsInteraction, PolicyDecision::ApprovalPending);
}
