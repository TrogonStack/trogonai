use super::*;

fn act(agent: &str, next: Option<Act>) -> Act {
    Act {
        agent: agent.to_string(),
        act: next.map(Box::new),
    }
}

#[test]
fn is_valid_agent_identifier_accepts_draft_examples() {
    assert!(is_valid_agent_identifier("aauth:assistant-v2@agent.example"));
    assert!(is_valid_agent_identifier("aauth:planner.7f3c@vendor.example"));
    assert!(is_valid_agent_identifier("aauth:planner.7f3c+search1@vendor.example"));
}

#[test]
fn is_valid_agent_identifier_rejects_draft_invalid_examples() {
    assert!(!is_valid_agent_identifier("My Agent@agent.example"));
    assert!(!is_valid_agent_identifier("@agent.example"));
    assert!(!is_valid_agent_identifier("agent@http://agent.example"));
}

#[test]
fn is_valid_agent_identifier_rejects_missing_scheme() {
    assert!(!is_valid_agent_identifier("assistant@agent.example"));
}

#[test]
fn is_valid_agent_identifier_rejects_empty_local() {
    assert!(!is_valid_agent_identifier("aauth:@agent.example"));
}

#[test]
fn is_valid_agent_identifier_rejects_local_over_255_bytes() {
    let long_local = "a".repeat(256);
    let id = format!("aauth:{long_local}@agent.example");
    assert!(!is_valid_agent_identifier(&id));
}

#[test]
fn verify_act_chain_accepts_single_hop() {
    let a = act("aauth:asst@agent.example", None);
    verify_act_chain(&a).expect("single hop is valid");
}

#[test]
fn verify_act_chain_accepts_nested_sub_agent_inside_chain_example() {
    // Mirrors the draft's "A sub-agent inside a chain" example.
    let a = act(
        "aauth:booking@booking.example",
        Some(act("aauth:asst@agent.example", None)),
    );
    verify_act_chain(&a).expect("nested chain is valid");
}

#[test]
fn verify_act_chain_rejects_invalid_identifier_at_root() {
    let a = act("not-a-valid-identifier", None);
    let err = verify_act_chain(&a).unwrap_err();
    assert!(matches!(err, DelegationError::InvalidAgentIdentifier { depth: 0, .. }));
}

#[test]
fn verify_act_chain_rejects_invalid_identifier_when_nested() {
    let a = act("aauth:booking@booking.example", Some(act("bad identifier", None)));
    let err = verify_act_chain(&a).unwrap_err();
    assert!(matches!(err, DelegationError::InvalidAgentIdentifier { depth: 1, .. }));
}

#[test]
fn verify_act_chain_rejects_too_deep_chain() {
    let mut current = act("aauth:leaf@agent.example", None);
    for i in 0..MAX_CHAIN_DEPTH {
        current = act(&format!("aauth:hop{i}@agent.example"), Some(current));
    }
    let err = verify_act_chain(&current).unwrap_err();
    assert!(matches!(err, DelegationError::TooDeep));
}

#[test]
fn flatten_act_chain_orders_upstream_first() {
    let a = act(
        "aauth:booking@booking.example",
        Some(act("aauth:asst@agent.example", None)),
    );
    let flat = flatten_act_chain(&a);
    assert_eq!(
        flat,
        vec![
            FlattenedActEntry {
                depth: 0,
                agent: "aauth:booking@booking.example".into()
            },
            FlattenedActEntry {
                depth: 1,
                agent: "aauth:asst@agent.example".into()
            },
        ]
    );
}

#[test]
fn flatten_act_chain_single_hop() {
    let a = act("aauth:asst@agent.example", None);
    let flat = flatten_act_chain(&a);
    assert_eq!(flat.len(), 1);
    assert_eq!(flat[0].depth, 0);
}

#[test]
fn delegation_error_display_messages_are_distinct() {
    let cases = [
        format!(
            "{}",
            DelegationError::InvalidAgentIdentifier {
                depth: 0,
                agent: "x".into()
            }
        ),
        format!("{}", DelegationError::TooDeep),
    ];
    assert_ne!(cases[0], cases[1]);
}
