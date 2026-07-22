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
