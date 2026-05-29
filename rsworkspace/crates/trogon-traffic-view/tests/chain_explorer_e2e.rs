use chrono::Utc;
use trogon_traffic_view::chain_explorer::{ChainExplorer, RenderOpts, render_tree};
use trogon_traffic_view::event::{ActChainHop, TrafficEvent, TrafficSource};

fn hop(sub: &str, agent_id: &str) -> ActChainHop {
    ActChainHop {
        sub: sub.into(),
        agent_id: Some(agent_id.into()),
        wkl: format!("spiffe://acme.local/ns/prod/sa/{agent_id}"),
        iat: 1_748_341_200,
    }
}

fn fixture_event(
    request_id: &str,
    act_chain: Option<Vec<ActChainHop>>,
    caller: &str,
    target: &str,
    outcome: &str,
) -> TrafficEvent {
    TrafficEvent {
        event_id: format!("evt-{request_id}"),
        ts: Utc::now(),
        tenant: "acme".into(),
        caller_sub: Some(caller.into()),
        caller_wkl: None,
        target_aud: Some(target.into()),
        purpose: Some("incident.response".into()),
        scope: None,
        outcome: outcome.into(),
        reason: None,
        act_chain,
        request_id: Some(request_id.into()),
        session_id: Some("trace-fixture".into()),
        source: TrafficSource::Gateway,
    }
}

#[test]
fn chain_explorer_e2e_renders_delegation_tree() {
    let originator = hop("user:alice@acme.com", "acme/triage-router");
    let router = hop("agent:acme/triage-router", "acme/triage-router");
    let oncall = hop("agent:acme/oncall-agent", "acme/oncall-agent");

    let events = vec![
        fixture_event(
            "req-root",
            Some(vec![]),
            "acme/triage-router",
            "urn:trogon:a2a:agent:acme:oncall-agent",
            "allow",
        ),
        fixture_event(
            "req-sts",
            Some(vec![originator.clone()]),
            "acme/triage-router",
            "urn:trogon:a2a:agent:acme:oncall-agent",
            "allow",
        ),
        fixture_event(
            "req-tool-allow",
            Some(vec![originator.clone(), router.clone()]),
            "acme/oncall-agent",
            "urn:trogon:backend:fs",
            "allow",
        ),
        fixture_event(
            "req-tool-deny",
            Some(vec![originator.clone(), router.clone(), oncall.clone()]),
            "acme/oncall-agent",
            "urn:trogon:backend:secrets",
            "deny",
        ),
        fixture_event(
            "req-leaf",
            Some(vec![originator, router, oncall]),
            "acme/oncall-agent",
            "urn:trogon:backend:notify",
            "allow",
        ),
    ];

    let tree = ChainExplorer::build_tree("req-root", events).expect("build tree");

    assert_eq!(tree.fanout(), 5);
    assert!(tree.depth() >= 2);

    let rendered = render_tree(&tree, &RenderOpts::default());
    assert!(rendered.contains("acme/triage-router"));
    assert!(rendered.contains("acme/oncall-agent"));
    assert!(rendered.contains("urn:trogon:backend:fs"));
    assert!(rendered.contains("urn:trogon:backend:secrets"));
    assert!(rendered.contains("allow"));
    assert!(rendered.contains("deny"));
}
