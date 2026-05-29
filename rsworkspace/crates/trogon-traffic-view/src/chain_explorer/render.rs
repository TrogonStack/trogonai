use super::tree::{ChainNode, ChainTree};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderOpts {
    pub branch: String,
    pub indent: String,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self {
            branch: "└── ".into(),
            indent: "    ".into(),
        }
    }
}

pub fn render_tree(tree: &ChainTree, opts: &RenderOpts) -> String {
    let mut lines = Vec::new();
    render_node(&tree.root, "", true, opts, &mut lines);
    lines.join("\n")
}

fn render_node(node: &ChainNode, prefix: &str, is_last: bool, opts: &RenderOpts, lines: &mut Vec<String>) {
    let connector = if prefix.is_empty() {
        String::new()
    } else if is_last {
        format!("{prefix}{}", opts.branch)
    } else {
        format!("{prefix}├── ")
    };
    lines.push(format!("{connector}{}", format_hop(&node.event)));

    let child_prefix = if prefix.is_empty() {
        String::new()
    } else if is_last {
        format!("{prefix}{}", opts.indent)
    } else {
        format!("{prefix}│   ")
    };
    let child_count = node.children.len();
    for (index, child) in node.children.iter().enumerate() {
        render_node(
            child,
            &child_prefix,
            index + 1 == child_count,
            opts,
            lines,
        );
    }
}

fn format_hop(event: &crate::event::TrafficEvent) -> String {
    format!(
        "{} -> {}  {}",
        event.caller_sub.as_deref().unwrap_or("-"),
        event.target_aud.as_deref().unwrap_or("-"),
        event.outcome
    )
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::chain_explorer::tree::ChainTree;
    use crate::event::{ActChainHop, TrafficEvent, TrafficSource};

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
    fn render_includes_caller_target_and_outcome() {
        let hop_a = hop("acme/agent-a");
        let events = vec![
            test_event(
                "req-a",
                Some(vec![]),
                "acme/router",
                "urn:agent:oncall",
                "allow",
            ),
            test_event(
                "req-b",
                Some(vec![hop_a]),
                "acme/oncall",
                "urn:tool:db_query",
                "deny",
            ),
        ];
        let tree = ChainTree::from_events("req-a", events).expect("tree");
        let rendered = render_tree(&tree, &RenderOpts::default());
        assert!(rendered.contains("acme/router"));
        assert!(rendered.contains("urn:agent:oncall"));
        assert!(rendered.contains("acme/oncall"));
        assert!(rendered.contains("urn:tool:db_query"));
        assert!(rendered.contains("allow"));
        assert!(rendered.contains("deny"));
    }
}
