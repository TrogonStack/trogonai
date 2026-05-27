use trogon_mcp_gateway::act_chain::ActChainEntry;

#[test]
fn act_chain_entry_is_the_shared_type() {
    static_assertions::assert_type_eq_all!(ActChainEntry, trogon_identity_types::ActChainEntry);
}

#[test]
fn act_chain_entry_json_round_trip_via_gateway_reexport() {
    let shared = trogon_identity_types::ActChainEntry {
        sub: "trogon-sts".into(),
        agent_id: Some("acme/agent".into()),
        wkl: Some("spiffe://acme/sa/agent".into()),
        iat: 1_700_000_000,
    };
    let json = serde_json::to_string(&shared).expect("serialize shared type");
    let via_gateway: ActChainEntry = serde_json::from_str(&json).expect("deserialize via gateway re-export");
    assert_eq!(via_gateway, shared);
}
