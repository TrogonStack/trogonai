use super::*;

#[test]
fn act_serde_round_trip_single_hop() {
    let act = Act {
        agent: "aauth:asst@agent.example".into(),
        act: None,
    };
    let json = serde_json::to_value(&act).unwrap();
    assert_eq!(json["agent"], "aauth:asst@agent.example");
    assert!(json.get("act").is_none());
    let back: Act = serde_json::from_value(json).unwrap();
    assert_eq!(back, act);
}

#[test]
fn act_serde_round_trip_nested_chain() {
    let act = Act {
        agent: "aauth:booking@booking.example".into(),
        act: Some(Box::new(Act {
            agent: "aauth:asst@agent.example".into(),
            act: None,
        })),
    };
    let json = serde_json::to_string(&act).unwrap();
    let back: Act = serde_json::from_str(&json).unwrap();
    assert_eq!(back, act);
    assert_eq!(back.act.unwrap().agent, "aauth:asst@agent.example");
}

#[test]
fn act_matches_delegation_chain_example_sub_agent_inside_chain() {
    let raw = serde_json::json!({
        "agent": "aauth:booking@booking.example",
        "act": { "agent": "aauth:asst@agent.example" }
    });
    let act: Act = serde_json::from_value(raw).unwrap();
    assert_eq!(act.agent, "aauth:booking@booking.example");
    assert_eq!(act.act.as_ref().unwrap().agent, "aauth:asst@agent.example");
}
