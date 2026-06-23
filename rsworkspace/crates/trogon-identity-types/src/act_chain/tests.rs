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
