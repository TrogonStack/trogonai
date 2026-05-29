use std::collections::HashMap;

use crate::event::{ActChainHop, TrafficEvent};

use super::errors::ChainExplorerError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainNode {
    pub event: TrafficEvent,
    pub children: Vec<ChainNode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainTree {
    pub root: ChainNode,
}

impl ChainTree {
    pub fn from_events(
        root_request_id: &str,
        events: Vec<TrafficEvent>,
    ) -> Result<Self, ChainExplorerError> {
        if events.is_empty() {
            return Err(ChainExplorerError::RootNotFound);
        }

        let by_request_id: HashMap<String, &TrafficEvent> = events
            .iter()
            .filter_map(|event| {
                let request_id = event.request_id.as_deref()?;
                Some((request_id.to_owned(), event))
            })
            .collect();

        for event in &events {
            if event.request_id.is_none() {
                return Err(ChainExplorerError::MalformedChain {
                    request_id: event.event_id.clone(),
                    reason: "missing request_id".into(),
                });
            }
        }

        let root_event = match find_root(root_request_id, &events) {
            Ok(root) => root,
            Err(ChainExplorerError::RootNotFound) => {
                if let Some(orphan) = first_orphan(&events)? {
                    return Err(ChainExplorerError::OrphanedEvent {
                        request_id: orphan,
                    });
                }
                return Err(ChainExplorerError::RootNotFound);
            }
            Err(error) => return Err(error),
        };
        let root_request_id = root_event
            .request_id
            .as_deref()
            .expect("root validated above");

        let mut children_by_parent: HashMap<String, Vec<String>> = HashMap::new();
        for event in &events {
            let request_id = event.request_id.as_deref().expect("validated above");
            if request_id == root_request_id {
                continue;
            }

            let parent_request_id = parent_request_id(event, &events)?;
            if !by_request_id.contains_key(parent_request_id) {
                return Err(ChainExplorerError::OrphanedEvent {
                    request_id: request_id.to_owned(),
                });
            }
            children_by_parent
                .entry(parent_request_id.to_owned())
                .or_default()
                .push(request_id.to_owned());
        }

        let root = build_node(root_request_id, &by_request_id, &mut children_by_parent);
        Ok(Self { root })
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        max_depth(&self.root)
    }

    #[must_use]
    pub fn fanout(&self) -> usize {
        count_nodes(&self.root)
    }
}

fn chain_len(event: &TrafficEvent) -> usize {
    event.act_chain.as_ref().map_or(0, Vec::len)
}

fn chain_prefix(event: &TrafficEvent) -> Option<&[ActChainHop]> {
    let chain = event.act_chain.as_ref()?;
    if chain.is_empty() {
        return None;
    }
    Some(&chain[..chain.len() - 1])
}

fn parent_request_id<'a>(
    event: &TrafficEvent,
    events: &'a [TrafficEvent],
) -> Result<&'a str, ChainExplorerError> {
    let request_id = event.request_id.as_deref().unwrap_or(&event.event_id);
    let Some(prefix) = chain_prefix(event) else {
        return Err(ChainExplorerError::MalformedChain {
            request_id: request_id.to_owned(),
            reason: "non-root event has empty act_chain".into(),
        });
    };

    let mut matches = events.iter().filter(|candidate| {
        candidate
            .act_chain
            .as_ref()
            .is_some_and(|chain| chain.as_slice() == prefix)
    });

    let parent = matches.next().ok_or_else(|| ChainExplorerError::OrphanedEvent {
        request_id: request_id.to_owned(),
    })?;

    if matches.next().is_some() {
        return Err(ChainExplorerError::MalformedChain {
            request_id: request_id.to_owned(),
            reason: "ambiguous act_chain prefix matches multiple parents".into(),
        });
    }

    parent.request_id.as_deref().ok_or_else(|| ChainExplorerError::MalformedChain {
        request_id: request_id.to_owned(),
        reason: "parent event missing request_id".into(),
    })
}

fn first_orphan(events: &[TrafficEvent]) -> Result<Option<String>, ChainExplorerError> {
    for event in events {
        let request_id = match event.request_id.as_deref() {
            Some(request_id) => request_id,
            None => continue,
        };
        if chain_len(event) == 0 {
            continue;
        }
        if parent_request_id(event, events).is_err() {
            return Ok(Some(request_id.to_owned()));
        }
    }
    Ok(None)
}

fn find_root<'a>(
    root_request_id: &str,
    events: &'a [TrafficEvent],
) -> Result<&'a TrafficEvent, ChainExplorerError> {
    if let Some(root) = events
        .iter()
        .find(|event| event.request_id.as_deref() == Some(root_request_id))
    {
        return Ok(root);
    }

    let mut empty_chain_roots = events.iter().filter(|event| chain_len(event) == 0);
    let Some(root) = empty_chain_roots.next() else {
        return Err(ChainExplorerError::RootNotFound);
    };
    if empty_chain_roots.next().is_some() {
        return Err(ChainExplorerError::MalformedChain {
            request_id: root_request_id.to_owned(),
            reason: "multiple root events with empty act_chain".into(),
        });
    }
    Ok(root)
}

fn build_node(
    request_id: &str,
    by_request_id: &HashMap<String, &TrafficEvent>,
    children_by_parent: &mut HashMap<String, Vec<String>>,
) -> ChainNode {
    let event = by_request_id
        .get(request_id)
        .copied()
        .expect("request_id present in index")
        .clone();
    let child_ids = children_by_parent.remove(request_id).unwrap_or_default();
    let children = child_ids
        .into_iter()
        .map(|child_id| build_node(&child_id, by_request_id, children_by_parent))
        .collect();
    ChainNode { event, children }
}

fn max_depth(node: &ChainNode) -> usize {
    if node.children.is_empty() {
        1
    } else {
        1 + node.children.iter().map(max_depth).max().unwrap_or(0)
    }
}

fn count_nodes(node: &ChainNode) -> usize {
    1 + node
        .children
        .iter()
        .map(count_nodes)
        .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::event::TrafficSource;

    fn hop(sub: &str) -> ActChainHop {
        ActChainHop {
            sub: sub.into(),
            agent_id: Some(sub.into()),
            wkl: format!("spiffe://acme.local/{sub}"),
            iat: 1_748_341_200,
        }
    }

    fn test_event(
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
            purpose: None,
            scope: None,
            outcome: outcome.into(),
            reason: None,
            act_chain,
            request_id: Some(request_id.into()),
            session_id: None,
            source: TrafficSource::Gateway,
        }
    }

    #[test]
    fn empty_input_returns_root_not_found() {
        let result = ChainTree::from_events("req-a", vec![]);
        assert_eq!(result, Err(ChainExplorerError::RootNotFound));
    }

    #[test]
    fn single_root_has_depth_one_and_fanout_one() {
        let events = vec![test_event(
            "req-a",
            Some(vec![]),
            "acme/agent-a",
            "urn:backend:a",
            "allow",
        )];
        let tree = ChainTree::from_events("req-a", events).expect("tree");
        assert_eq!(tree.depth(), 1);
        assert_eq!(tree.fanout(), 1);
    }

    #[test]
    fn linear_chain_has_depth_three() {
        let hop_a = hop("acme/agent-a");
        let hop_b = hop("acme/agent-b");
        let events = vec![
            test_event(
                "req-a",
                Some(vec![]),
                "acme/agent-a",
                "urn:backend:a",
                "allow",
            ),
            test_event(
                "req-b",
                Some(vec![hop_a.clone()]),
                "acme/agent-b",
                "urn:backend:b",
                "allow",
            ),
            test_event(
                "req-c",
                Some(vec![hop_a, hop_b]),
                "acme/agent-c",
                "urn:backend:c",
                "deny",
            ),
        ];
        let tree = ChainTree::from_events("req-a", events).expect("tree");
        assert_eq!(tree.depth(), 3);
        assert_eq!(tree.fanout(), 3);
    }

    #[test]
    fn fanout_tree_has_four_nodes_and_depth_two() {
        let hop_a = hop("acme/agent-a");
        let events = vec![
            test_event(
                "req-a",
                Some(vec![]),
                "acme/agent-a",
                "urn:backend:a",
                "allow",
            ),
            test_event(
                "req-b",
                Some(vec![hop_a.clone()]),
                "acme/agent-b",
                "urn:backend:b",
                "allow",
            ),
            test_event(
                "req-c",
                Some(vec![hop_a.clone()]),
                "acme/agent-c",
                "urn:backend:c",
                "allow",
            ),
            test_event(
                "req-d",
                Some(vec![hop_a]),
                "acme/agent-d",
                "urn:backend:d",
                "deny",
            ),
        ];
        let tree = ChainTree::from_events("req-a", events).expect("tree");
        assert_eq!(tree.depth(), 2);
        assert_eq!(tree.fanout(), 4);
        assert_eq!(tree.root.children.len(), 3);
    }

    #[test]
    fn orphan_when_parent_prefix_missing() {
        let hop_a = hop("acme/agent-a");
        let events = vec![test_event(
            "req-b",
            Some(vec![hop_a]),
            "acme/agent-b",
            "urn:backend:b",
            "allow",
        )];
        let result = ChainTree::from_events("req-a", events);
        assert_eq!(
            result,
            Err(ChainExplorerError::OrphanedEvent {
                request_id: "req-b".into()
            })
        );
    }
}
