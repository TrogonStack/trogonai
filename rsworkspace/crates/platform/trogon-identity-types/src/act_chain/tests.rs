use super::*;

#[test]
fn loop_detection_finds_duplicate_agent_wkl() {
    let chain = vec![
        ActChainEntry {
            sub: "a".into(),
            agent_id: Some("acme/agent".into()),
            wkl: Some("spiffe://acme/sa/a".into()),
            iat: 1,
        },
        ActChainEntry {
            sub: "b".into(),
            agent_id: Some("acme/agent".into()),
            wkl: Some("spiffe://acme/sa/a".into()),
            iat: 2,
        },
    ];
    assert!(act_chain_has_loop(&chain));
}

#[test]
fn loop_detection_returns_false_for_unique_chain() {
    let chain = vec![
        ActChainEntry {
            sub: "a".into(),
            agent_id: Some("agent-1".into()),
            wkl: Some("spiffe://acme/sa/a".into()),
            iat: 1,
        },
        ActChainEntry {
            sub: "b".into(),
            agent_id: Some("agent-2".into()),
            wkl: Some("spiffe://acme/sa/b".into()),
            iat: 2,
        },
    ];
    assert!(!act_chain_has_loop(&chain));
}

#[test]
fn parse_act_chain_deserializes_json_array() {
    let raw = r#"[{"sub":"alice","agent_id":"acme/agent","iat":1}]"#;
    let chain = parse_act_chain(raw).unwrap();
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].sub, "alice");
    assert_eq!(chain[0].agent_id.as_deref(), Some("acme/agent"));
    assert!(chain[0].wkl.is_none());
}

#[test]
fn loop_detection_skips_entries_with_both_agent_and_wkl_empty() {
    let chain = vec![
        ActChainEntry {
            sub: "a".into(),
            agent_id: None,
            wkl: None,
            iat: 1,
        },
        ActChainEntry {
            sub: "b".into(),
            agent_id: None,
            wkl: None,
            iat: 2,
        },
    ];
    assert!(!act_chain_has_loop(&chain));
}
