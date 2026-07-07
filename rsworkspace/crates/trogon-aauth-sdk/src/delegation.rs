//! Delegation-chain inspection helpers over `Act`, per draft "Agent
//! Delegation" / "Delegation Chain".
//!
//! `Act` lives in `trogon-identity-types::aauth::delegation`, a different
//! crate from this module's own name -- this is an SDK-local extension trait,
//! not a redefinition of the shared type. A trait impl for a foreign type is
//! fine as long as the trait itself is local, which is why this is a trait
//! rather than inherent methods.

use trogon_identity_types::aauth::Act;

/// Extension methods for inspecting an `act` delegation chain without
/// hand-rolling the recursive walk at every call site.
pub trait ActChainExt {
    /// The full chain of agent identifiers, immediate-upstream first, root
    /// (the entry with no further nested `act`) last.
    fn chain(&self) -> Vec<String>;

    /// Number of hops in the chain, i.e. `chain().len()`.
    fn depth(&self) -> usize;

    /// Whether `agent_id` appears anywhere in the chain.
    fn contains_agent(&self, agent_id: &str) -> bool;
}

impl ActChainExt for Act {
    fn chain(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut current = Some(self);
        while let Some(act) = current {
            out.push(act.agent.clone());
            current = act.act.as_deref();
        }
        out
    }

    fn depth(&self) -> usize {
        self.chain().len()
    }

    fn contains_agent(&self, agent_id: &str) -> bool {
        self.chain().iter().any(|agent| agent == agent_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nested_act() -> Act {
        Act {
            agent: "aauth:booking@booking.example".to_string(),
            act: Some(Box::new(Act {
                agent: "aauth:asst@agent.example".to_string(),
                act: Some(Box::new(Act {
                    agent: "aauth:planner@agent.example".to_string(),
                    act: None,
                })),
            })),
        }
    }

    #[test]
    fn chain_lists_immediate_upstream_first_root_last() {
        let act = nested_act();
        assert_eq!(
            act.chain(),
            vec![
                "aauth:booking@booking.example".to_string(),
                "aauth:asst@agent.example".to_string(),
                "aauth:planner@agent.example".to_string(),
            ]
        );
    }

    #[test]
    fn depth_counts_hops() {
        let act = nested_act();
        assert_eq!(act.depth(), 3);

        let single = Act {
            agent: "aauth:asst@agent.example".to_string(),
            act: None,
        };
        assert_eq!(single.depth(), 1);
    }

    #[test]
    fn contains_agent_checks_membership_anywhere_in_chain() {
        let act = nested_act();
        assert!(act.contains_agent("aauth:planner@agent.example"));
        assert!(act.contains_agent("aauth:booking@booking.example"));
        assert!(!act.contains_agent("aauth:someone-else@agent.example"));
    }
}
