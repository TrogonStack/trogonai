use super::*;
use crate::fixtures::{name, scope, token};

#[test]
fn exposes_project_and_actor() {
    let scope = scope();
    assert_eq!(scope.project().as_str(), "project_1");
    assert_eq!(
        scope.actor(),
        &Actor::Agent {
            agent_id: name("agent-7")
        }
    );
}

#[test]
fn builds_with_correlation_and_a_delegation_chain() {
    let scope = RecordScope::new(name("t"), token("p"), Actor::Unattributed)
        .with_act_chain(vec![name("a"), name("b")])
        .unwrap()
        .with_trace_id(TraceId::new("4bf92f3577b34da6a3ce929d0e0e4736").unwrap())
        .with_request_id(name("req-1"));
    assert_eq!(scope.act_chain.len(), 2);
    assert!(scope.trace_id.is_some());
    assert_eq!(scope.request_id, Some(name("req-1")));
}

#[test]
fn bounds_the_act_chain() {
    let at_limit = vec![name("a"); MAX_ACT_CHAIN_ENTRIES];
    assert!(scope().with_act_chain(at_limit).is_ok());
    let over = vec![name("a"); MAX_ACT_CHAIN_ENTRIES + 1];
    assert_eq!(
        scope().with_act_chain(over),
        Err(ActChainTooLongError(MAX_ACT_CHAIN_ENTRIES + 1))
    );
}

#[test]
fn the_error_message_states_the_bound() {
    assert_eq!(
        ActChainTooLongError(17).to_string(),
        "act chain has 17 entries, over the 16-entry limit"
    );
}
