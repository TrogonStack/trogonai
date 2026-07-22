use super::*;

fn agent(s: &str) -> A2aAgentId {
    A2aAgentId::new(s).expect("nats-safe test agent id")
}

#[test]
fn session_key_round_trips_sub_and_account() {
    let key = SpiceDbSessionKey::new("alice", "acme");
    assert_eq!(key.sub(), "alice");
    assert_eq!(key.account(), "acme");
}

#[test]
fn session_key_equality_distinguishes_sub_vs_account() {
    // The cache uses the pair as the lookup key — swapping sub and
    // account between two callers must NOT collide on the same
    // cached outcome.
    let a = SpiceDbSessionKey::new("alice", "acme");
    let b = SpiceDbSessionKey::new("acme", "alice");
    assert_ne!(a, b);
}

#[test]
fn agent_view_tuple_resource_id_matches_agent_id() {
    let t = AgentViewTuple::new(agent("planner"));
    assert_eq!(t.resource_id(), "planner");
    assert_eq!(t.agent_id().as_str(), "planner");
}

#[test]
fn filter_marks_allowed_agents_visible() {
    let ids = vec![agent("a1"), agent("a2")];
    let outcomes = vec![AgentViewCheckOutcome::Allowed, AgentViewCheckOutcome::Allowed];
    let filtered = filter_agents_by_view(&ids, &outcomes).expect("equal-length zip");
    assert_eq!(
        filtered,
        vec![
            DiscoveryAgentFilterOutcome::Visible(agent("a1")),
            DiscoveryAgentFilterOutcome::Visible(agent("a2")),
        ]
    );
}

#[test]
fn filter_hides_denied_agents_with_reason() {
    let ids = vec![agent("a1")];
    let outcomes = vec![AgentViewCheckOutcome::Denied];
    let filtered = filter_agents_by_view(&ids, &outcomes).expect("equal-length zip");
    assert_eq!(
        filtered,
        vec![DiscoveryAgentFilterOutcome::Hidden {
            agent_id: agent("a1"),
            reason: DiscoveryHiddenReason::Denied,
        }]
    );
}

#[test]
fn filter_hides_transport_errors_with_distinct_reason() {
    // A transport error must surface as a distinct hidden reason
    // from a policy denial — operators need to tell "we couldn't
    // ask the authorizer" from "policy said no".
    let ids = vec![agent("a1")];
    let outcomes = vec![AgentViewCheckOutcome::TransportError];
    let filtered = filter_agents_by_view(&ids, &outcomes).expect("equal-length zip");
    assert_eq!(
        filtered,
        vec![DiscoveryAgentFilterOutcome::Hidden {
            agent_id: agent("a1"),
            reason: DiscoveryHiddenReason::TransportError,
        }]
    );
}

#[test]
fn filter_preserves_input_order() {
    // The caller order maps to the discovery response order; the
    // filter must not reorder.
    let ids = vec![agent("z"), agent("a"), agent("m")];
    let outcomes = vec![
        AgentViewCheckOutcome::Allowed,
        AgentViewCheckOutcome::Denied,
        AgentViewCheckOutcome::Allowed,
    ];
    let filtered = filter_agents_by_view(&ids, &outcomes).expect("equal-length zip");
    assert_eq!(filtered.len(), 3);
    assert!(matches!(filtered[0], DiscoveryAgentFilterOutcome::Visible(ref a) if a.as_str() == "z"));
    assert!(
        matches!(filtered[1], DiscoveryAgentFilterOutcome::Hidden { ref agent_id, .. } if agent_id.as_str() == "a")
    );
    assert!(matches!(filtered[2], DiscoveryAgentFilterOutcome::Visible(ref a) if a.as_str() == "m"));
}

#[test]
fn filter_fails_closed_on_length_mismatch() {
    // A caller bug that supplies fewer outcomes than agents must
    // not silently truncate the response: failing closed surfaces
    // the mismatch loudly instead of letting an orphaned agent
    // slip through as `Visible` (zip would just stop early).
    let ids = vec![agent("a1"), agent("a2")];
    let outcomes = vec![AgentViewCheckOutcome::Allowed];
    let err = filter_agents_by_view(&ids, &outcomes).expect_err("must reject mismatch");
    assert_eq!(err, AgentViewFilterError::LengthMismatch { agents: 2, outcomes: 1 });
}

#[test]
fn filter_fails_closed_on_extra_outcomes() {
    let ids = vec![agent("a1")];
    let outcomes = vec![AgentViewCheckOutcome::Allowed, AgentViewCheckOutcome::Denied];
    let err = filter_agents_by_view(&ids, &outcomes).expect_err("must reject mismatch");
    assert_eq!(err, AgentViewFilterError::LengthMismatch { agents: 1, outcomes: 2 });
}

#[test]
fn agent_view_filter_error_renders_expected_message() {
    let err = AgentViewFilterError::LengthMismatch { agents: 3, outcomes: 2 };
    assert_eq!(
        err.to_string(),
        "agent_ids and outcomes must have the same length (agents=3, outcomes=2)"
    );
}

#[test]
fn outcome_enums_compare_by_variant() {
    assert_eq!(AgentViewCheckOutcome::Allowed, AgentViewCheckOutcome::Allowed);
    assert_ne!(AgentViewCheckOutcome::Allowed, AgentViewCheckOutcome::Denied);
    assert_eq!(DiscoveryHiddenReason::Denied, DiscoveryHiddenReason::Denied);
    assert_ne!(DiscoveryHiddenReason::Denied, DiscoveryHiddenReason::TransportError);
}
