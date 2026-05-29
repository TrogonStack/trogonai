use std::collections::HashMap;

use crate::event::TrafficEvent;

use super::errors::{validate_n, TopNError};
use super::types::{DeepestChain, LongestChainByTenant, MostActiveAgent, MostDeniedAgent};

fn grouping_key(event: &TrafficEvent) -> Option<&str> {
    if let Some(chain) = event.act_chain.as_ref() {
        if let Some(root) = chain.first() {
            if let Some(agent_id) = root.agent_id.as_deref() {
                return Some(agent_id);
            }
        }
    }
    event.caller_sub.as_deref()
}

fn chain_depth(event: &TrafficEvent) -> usize {
    event.act_chain.as_ref().map_or(0, Vec::len)
}

pub fn most_active_agents(
    events: &[TrafficEvent],
    n: usize,
) -> Result<Vec<MostActiveAgent>, TopNError> {
    validate_n(n)?;

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for event in events {
        if let Some(key) = grouping_key(event) {
            *counts.entry(key).or_default() += 1;
        }
    }

    let mut ranked: Vec<MostActiveAgent> = counts
        .into_iter()
        .map(|(agent_id, event_count)| MostActiveAgent {
            agent_id: agent_id.to_owned(),
            event_count,
        })
        .collect();

    ranked.sort_by(|left, right| {
        right
            .event_count
            .cmp(&left.event_count)
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
    ranked.truncate(n);
    Ok(ranked)
}

pub fn most_denied_agents(
    events: &[TrafficEvent],
    n: usize,
) -> Result<Vec<MostDeniedAgent>, TopNError> {
    validate_n(n)?;

    let mut deny_counts: HashMap<&str, usize> = HashMap::new();
    let mut reason_counts: HashMap<&str, HashMap<String, usize>> = HashMap::new();

    for event in events {
        if event.outcome == "allow" {
            continue;
        }
        let Some(key) = grouping_key(event) else {
            continue;
        };
        *deny_counts.entry(key).or_default() += 1;
        if let Some(reason) = event.reason.as_ref() {
            *reason_counts
                .entry(key)
                .or_default()
                .entry(reason.clone())
                .or_default() += 1;
        }
    }

    let mut ranked: Vec<MostDeniedAgent> = deny_counts
        .into_iter()
        .map(|(agent_id, deny_count)| {
            let top_reason = reason_counts.get(agent_id).and_then(|reasons| {
                let mut ranked: Vec<_> = reasons.iter().collect();
                ranked.sort_by(|(left_reason, left_count), (right_reason, right_count)| {
                    right_count
                        .cmp(left_count)
                        .then_with(|| left_reason.cmp(right_reason))
                });
                ranked.first().map(|(reason, _)| (*reason).clone())
            });
            MostDeniedAgent {
                agent_id: agent_id.to_owned(),
                deny_count,
                top_reason,
            }
        })
        .collect();

    ranked.sort_by(|left, right| {
        right
            .deny_count
            .cmp(&left.deny_count)
            .then_with(|| left.agent_id.cmp(&right.agent_id))
    });
    ranked.truncate(n);
    Ok(ranked)
}

pub fn deepest_chains(events: &[TrafficEvent], n: usize) -> Result<Vec<DeepestChain>, TopNError> {
    validate_n(n)?;

    let mut ranked: Vec<DeepestChain> = events
        .iter()
        .map(|event| DeepestChain {
            event_id: event.event_id.clone(),
            tenant: event.tenant.clone(),
            depth: chain_depth(event),
            ts: event.ts,
        })
        .collect();

    ranked.sort_by(|left, right| {
        right
            .depth
            .cmp(&left.depth)
            .then_with(|| right.ts.cmp(&left.ts))
    });
    ranked.truncate(n);
    Ok(ranked)
}

pub fn longest_chain_by_tenant(
    events: &[TrafficEvent],
) -> Result<Vec<LongestChainByTenant>, TopNError> {
    let mut best_by_tenant: HashMap<&str, LongestChainByTenant> = HashMap::new();

    for event in events {
        let depth = chain_depth(event);
        let candidate = LongestChainByTenant {
            tenant: event.tenant.clone(),
            event_id: event.event_id.clone(),
            depth,
            ts: event.ts,
        };

        best_by_tenant
            .entry(event.tenant.as_str())
            .and_modify(|current| {
                let replace = candidate.depth > current.depth
                    || (candidate.depth == current.depth && candidate.ts > current.ts);
                if replace {
                    *current = candidate.clone();
                }
            })
            .or_insert(candidate);
    }

    let mut ranked: Vec<LongestChainByTenant> = best_by_tenant.into_values().collect();
    ranked.sort_by(|left, right| left.tenant.cmp(&right.tenant));
    Ok(ranked)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::event::{ActChainHop, TrafficSource};

    fn hop(sub: &str, agent_id: Option<&str>) -> ActChainHop {
        ActChainHop {
            sub: sub.into(),
            agent_id: agent_id.map(str::to_owned),
            wkl: format!("spiffe://acme.local/{sub}"),
            iat: 1,
        }
    }

    fn event(
        event_id: &str,
        tenant: &str,
        caller_sub: Option<&str>,
        outcome: &str,
        reason: Option<&str>,
        act_chain: Option<Vec<ActChainHop>>,
        ts: &str,
    ) -> TrafficEvent {
        TrafficEvent {
            event_id: event_id.into(),
            ts: Utc.with_ymd_and_hms(2026, 5, 27, 14, 0, 0).unwrap()
                + chrono::Duration::seconds(
                    ts.parse::<i64>().expect("seconds offset from base"),
                ),
            tenant: tenant.into(),
            caller_sub: caller_sub.map(str::to_owned),
            caller_wkl: None,
            target_aud: None,
            purpose: None,
            scope: None,
            outcome: outcome.into(),
            reason: reason.map(str::to_owned),
            act_chain,
            request_id: None,
            session_id: None,
            source: TrafficSource::Gateway,
        }
    }

    #[test]
    fn most_active_agents_returns_top_n_by_count() {
        let events = [
            event("e1", "acme", Some("acme/agent-a"), "allow", None, None, "1"),
            event("e2", "acme", Some("acme/agent-a"), "allow", None, None, "2"),
            event("e3", "acme", Some("acme/agent-a"), "allow", None, None, "3"),
            event("e4", "acme", Some("acme/agent-a"), "allow", None, None, "4"),
            event("e5", "acme", Some("acme/agent-a"), "allow", None, None, "5"),
            event("e6", "acme", Some("acme/agent-b"), "allow", None, None, "6"),
            event("e7", "acme", Some("acme/agent-b"), "allow", None, None, "7"),
            event("e8", "acme", Some("acme/agent-b"), "allow", None, None, "8"),
            event("e9", "acme", Some("acme/agent-c"), "allow", None, None, "9"),
        ];

        let top = most_active_agents(&events, 2).expect("valid n");
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].agent_id, "acme/agent-a");
        assert_eq!(top[0].event_count, 5);
        assert_eq!(top[1].agent_id, "acme/agent-b");
        assert_eq!(top[1].event_count, 3);
    }

    #[test]
    fn most_active_agents_breaks_ties_alphabetically() {
        let events = [
            event("e1", "acme", Some("acme/agent-z"), "allow", None, None, "1"),
            event("e2", "acme", Some("acme/agent-a"), "allow", None, None, "2"),
            event("e3", "acme", Some("acme/agent-m"), "allow", None, None, "3"),
            event("e4", "acme", Some("acme/agent-m"), "allow", None, None, "4"),
        ];

        let top = most_active_agents(&events, 3).expect("valid n");
        assert_eq!(
            top.iter().map(|entry| entry.agent_id.as_str()).collect::<Vec<_>>(),
            vec!["acme/agent-m", "acme/agent-a", "acme/agent-z"]
        );
    }

    #[test]
    fn most_active_agents_prefers_root_agent_id_over_caller_sub() {
        let chain = vec![
            hop("agent:acme/router", Some("acme/router")),
            hop("agent:acme/worker", Some("acme/worker")),
        ];
        let events = [
            event(
                "e1",
                "acme",
                Some("acme/worker"),
                "allow",
                None,
                Some(chain.clone()),
                "1",
            ),
            event(
                "e2",
                "acme",
                Some("acme/worker"),
                "allow",
                None,
                Some(chain),
                "2",
            ),
        ];

        let top = most_active_agents(&events, 1).expect("valid n");
        assert_eq!(top[0].agent_id, "acme/router");
        assert_eq!(top[0].event_count, 2);
    }

    #[test]
    fn most_denied_agents_filters_allows_and_carries_top_reason() {
        let events = [
            event(
                "e1",
                "acme",
                Some("acme/agent-a"),
                "deny",
                Some("policy"),
                None,
                "1",
            ),
            event(
                "e2",
                "acme",
                Some("acme/agent-a"),
                "deny",
                Some("policy"),
                None,
                "2",
            ),
            event(
                "e3",
                "acme",
                Some("acme/agent-a"),
                "deny",
                Some("quota"),
                None,
                "3",
            ),
            event("e4", "acme", Some("acme/agent-a"), "allow", None, None, "4"),
            event(
                "e5",
                "acme",
                Some("acme/agent-b"),
                "deny",
                Some("scope"),
                None,
                "5",
            ),
        ];

        let top = most_denied_agents(&events, 2).expect("valid n");
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].agent_id, "acme/agent-a");
        assert_eq!(top[0].deny_count, 3);
        assert_eq!(top[0].top_reason.as_deref(), Some("policy"));
        assert_eq!(top[1].agent_id, "acme/agent-b");
        assert_eq!(top[1].deny_count, 1);
    }

    #[test]
    fn deepest_chains_orders_by_depth_then_ts_desc() {
        let shallow = vec![hop("agent:acme/a", Some("acme/a"))];
        let deep = vec![
            hop("agent:acme/a", Some("acme/a")),
            hop("agent:acme/b", Some("acme/b")),
            hop("agent:acme/c", Some("acme/c")),
        ];
        let events = [
            event(
                "e1",
                "acme",
                Some("acme/a"),
                "allow",
                None,
                Some(shallow.clone()),
                "1",
            ),
            event(
                "e2",
                "acme",
                Some("acme/c"),
                "allow",
                None,
                Some(deep.clone()),
                "2",
            ),
            event(
                "e3",
                "acme",
                Some("acme/c"),
                "allow",
                None,
                Some(deep),
                "3",
            ),
            event(
                "e4",
                "acme",
                Some("acme/a"),
                "allow",
                None,
                Some(shallow),
                "4",
            ),
        ];

        let top = deepest_chains(&events, 2).expect("valid n");
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].depth, 3);
        assert_eq!(top[0].event_id, "e3");
        assert_eq!(top[1].depth, 3);
        assert_eq!(top[1].event_id, "e2");
    }

    #[test]
    fn longest_chain_by_tenant_returns_one_entry_per_tenant() {
        let acme_chain = vec![
            hop("agent:acme/a", Some("acme/a")),
            hop("agent:acme/b", Some("acme/b")),
        ];
        let beta_chain = vec![hop("agent:beta/x", Some("beta/x"))];
        let events = [
            event(
                "e1",
                "acme",
                Some("acme/b"),
                "allow",
                None,
                Some(acme_chain),
                "1",
            ),
            event(
                "e2",
                "beta",
                Some("beta/x"),
                "allow",
                None,
                Some(beta_chain),
                "2",
            ),
            event("e3", "acme", Some("acme/a"), "allow", None, None, "3"),
        ];

        let rows = longest_chain_by_tenant(&events).expect("valid");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].tenant, "acme");
        assert_eq!(rows[0].depth, 2);
        assert_eq!(rows[1].tenant, "beta");
        assert_eq!(rows[1].depth, 1);
    }

    #[test]
    fn invalid_n_returns_error() {
        let events = [event("e1", "acme", Some("acme/a"), "allow", None, None, "1")];
        assert_eq!(
            most_active_agents(&events, 0),
            Err(TopNError::InvalidN)
        );
        assert_eq!(most_denied_agents(&events, 0), Err(TopNError::InvalidN));
        assert_eq!(deepest_chains(&events, 0), Err(TopNError::InvalidN));
    }
}
