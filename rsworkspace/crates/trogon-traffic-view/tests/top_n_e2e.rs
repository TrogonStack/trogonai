use chrono::{TimeZone, Utc};
use trogon_traffic_view::event::{ActChainHop, TrafficEvent, TrafficSource};
use trogon_traffic_view::top_n::TopNDashboards;

fn hop(sub: &str, agent_id: &str) -> ActChainHop {
    ActChainHop {
        sub: sub.into(),
        agent_id: Some(agent_id.into()),
        wkl: format!("spiffe://acme.local/{agent_id}"),
        iat: 1,
    }
}

fn fixture_events() -> Vec<TrafficEvent> {
    let base = Utc.with_ymd_and_hms(2026, 5, 27, 14, 0, 0).unwrap();
    vec![
        TrafficEvent {
            event_id: "evt-1".into(),
            ts: base + chrono::Duration::seconds(1),
            tenant: "acme".into(),
            caller_sub: Some("acme/oncall".into()),
            caller_wkl: None,
            target_aud: None,
            purpose: Some("incident.response".into()),
            scope: None,
            outcome: "allow".into(),
            reason: None,
            act_chain: Some(vec![
                hop("agent:acme/router", "acme/router"),
                hop("agent:acme/oncall", "acme/oncall"),
            ]),
            request_id: None,
            session_id: None,
            source: TrafficSource::Gateway,
        },
        TrafficEvent {
            event_id: "evt-2".into(),
            ts: base + chrono::Duration::seconds(2),
            tenant: "acme".into(),
            caller_sub: Some("acme/oncall".into()),
            caller_wkl: None,
            target_aud: None,
            purpose: Some("incident.response".into()),
            scope: None,
            outcome: "deny".into(),
            reason: Some("scope".into()),
            act_chain: Some(vec![
                hop("agent:acme/router", "acme/router"),
                hop("agent:acme/oncall", "acme/oncall"),
            ]),
            request_id: None,
            session_id: None,
            source: TrafficSource::Gateway,
        },
        TrafficEvent {
            event_id: "evt-3".into(),
            ts: base + chrono::Duration::seconds(3),
            tenant: "acme".into(),
            caller_sub: Some("acme/router".into()),
            caller_wkl: None,
            target_aud: None,
            purpose: None,
            scope: None,
            outcome: "allow".into(),
            reason: None,
            act_chain: Some(vec![hop("agent:acme/router", "acme/router")]),
            request_id: None,
            session_id: None,
            source: TrafficSource::Sts,
        },
        TrafficEvent {
            event_id: "evt-4".into(),
            ts: base + chrono::Duration::seconds(4),
            tenant: "acme".into(),
            caller_sub: Some("acme/oncall".into()),
            caller_wkl: None,
            target_aud: None,
            purpose: None,
            scope: None,
            outcome: "deny".into(),
            reason: Some("policy".into()),
            act_chain: Some(vec![
                hop("agent:acme/router", "acme/router"),
                hop("agent:acme/oncall", "acme/oncall"),
                hop("agent:acme/worker", "acme/worker"),
            ]),
            request_id: None,
            session_id: None,
            source: TrafficSource::Gateway,
        },
        TrafficEvent {
            event_id: "evt-5".into(),
            ts: base + chrono::Duration::seconds(5),
            tenant: "beta".into(),
            caller_sub: Some("beta/agent".into()),
            caller_wkl: None,
            target_aud: None,
            purpose: None,
            scope: None,
            outcome: "allow".into(),
            reason: None,
            act_chain: Some(vec![hop("agent:beta/agent", "beta/agent")]),
            request_id: None,
            session_id: None,
            source: TrafficSource::Gateway,
        },
        TrafficEvent {
            event_id: "evt-6".into(),
            ts: base + chrono::Duration::seconds(6),
            tenant: "beta".into(),
            caller_sub: Some("beta/agent".into()),
            caller_wkl: None,
            target_aud: None,
            purpose: None,
            scope: None,
            outcome: "deny".into(),
            reason: Some("quota".into()),
            act_chain: None,
            request_id: None,
            session_id: None,
            source: TrafficSource::Registry,
        },
        TrafficEvent {
            event_id: "evt-7".into(),
            ts: base + chrono::Duration::seconds(7),
            tenant: "acme".into(),
            caller_sub: Some("user:alice".into()),
            caller_wkl: None,
            target_aud: None,
            purpose: None,
            scope: None,
            outcome: "allow".into(),
            reason: None,
            act_chain: None,
            request_id: None,
            session_id: None,
            source: TrafficSource::Gateway,
        },
        TrafficEvent {
            event_id: "evt-8".into(),
            ts: base + chrono::Duration::seconds(8),
            tenant: "beta".into(),
            caller_sub: Some("beta/agent".into()),
            caller_wkl: None,
            target_aud: None,
            purpose: None,
            scope: None,
            outcome: "allow".into(),
            reason: None,
            act_chain: Some(vec![
                hop("agent:beta/agent", "beta/agent"),
                hop("agent:beta/helper", "beta/helper"),
            ]),
            request_id: None,
            session_id: None,
            source: TrafficSource::Gateway,
        },
    ]
}

#[test]
fn top_n_dashboards_e2e_shape() {
    let events = fixture_events();
    let dashboards = TopNDashboards::new(&events);

    let active = dashboards.most_active_agents(3).expect("active agents");
    assert_eq!(active.len(), 3);
    assert_eq!(active[0].agent_id, "acme/router");
    assert_eq!(active[0].event_count, 4);
    assert!(active.iter().all(|row| !row.agent_id.is_empty()));

    let denied = dashboards.most_denied_agents(2).expect("denied agents");
    assert_eq!(denied.len(), 2);
    assert_eq!(denied[0].agent_id, "acme/router");
    assert_eq!(denied[0].deny_count, 2);
    assert_eq!(denied[0].top_reason.as_deref(), Some("policy"));
    assert!(denied.iter().all(|row| row.deny_count > 0));

    let deepest = dashboards.deepest_chains(2).expect("deepest chains");
    assert_eq!(deepest.len(), 2);
    assert_eq!(deepest[0].depth, 3);
    assert_eq!(deepest[0].event_id, "evt-4");
    assert_eq!(deepest[1].depth, 2);
    assert!(deepest.iter().all(|row| !row.tenant.is_empty()));

    let by_tenant = dashboards.longest_chain_by_tenant().expect("by tenant");
    assert_eq!(by_tenant.len(), 2);
    assert_eq!(by_tenant[0].tenant, "acme");
    assert_eq!(by_tenant[0].depth, 3);
    assert_eq!(by_tenant[1].tenant, "beta");
    assert_eq!(by_tenant[1].depth, 2);
}
