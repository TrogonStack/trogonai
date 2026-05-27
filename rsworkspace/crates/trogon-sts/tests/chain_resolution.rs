mod support;

use support::*;
use trogon_identity_types::ActChainEntry;
use trogon_sts::chain_resolution::ChainResolutionMode;
use trogon_sts::registry::AgentRegistryRecord;

fn chain_agent(id: &str, wkl: &str) -> AgentRegistryRecord {
    AgentRegistryRecord {
        agent_id: id.into(),
        agent_version: "1.0.0".into(),
        agent_definition_digest: "sha256:abc".into(),
        owner_team: "platform".into(),
        allowed_workloads: vec![wkl.into()],
        allowed_tools: vec!["*".into()],
        allowed_audiences: vec!["urn:trogon:mcp:backend:acme:github".into()],
        allowed_purposes: vec!["oncall-incident-triage".into()],
        mesh_token_ttl_s: None,
        metadata: None,
        lifecycle_state: "active".into(),
    }
}

fn inbound_chain_before_append() -> Vec<ActChainEntry> {
    vec![
        ActChainEntry {
            sub: "user:alice".into(),
            agent_id: None,
            wkl: Some("sentinel:human".into()),
            iat: 1,
        },
        ActChainEntry {
            sub: "agent:acme/router".into(),
            agent_id: Some("acme/router".into()),
            wkl: Some("spiffe://acme.local/ns/prod/sa/router".into()),
            iat: 2,
        },
        ActChainEntry {
            sub: "agent:acme/worker".into(),
            agent_id: Some("acme/worker".into()),
            wkl: Some("spiffe://acme.local/ns/prod/sa/worker".into()),
            iat: 3,
        },
    ]
}

#[tokio::test]
async fn four_entry_chain_all_resolved_exchange_succeeds() {
    let keys = shared_test_keys();
    let records = [
        chain_agent("acme/router", "spiffe://acme.local/ns/prod/sa/router"),
        chain_agent("acme/worker", "spiffe://acme.local/ns/prod/sa/worker"),
        sample_registry_record(),
    ];
    let (service, _) = build_counting_service(
        keys,
        CountingRegistry::new(records),
        ChainResolutionMode::Strict,
    );

    let mut claims = bootstrap_claims(keys);
    claims["act_chain"] = serde_json::to_value(inbound_chain_before_append()).unwrap();
    let subject = mint_bootstrap_token(keys, claims);
    let response = service
        .handle(sample_exchange_request(&subject), None)
        .await
        .expect("exchange");
    assert!(!response.access_token.is_empty());
}

#[tokio::test]
async fn mid_chain_revocation_denies_with_offending_index() {
    let keys = shared_test_keys();
    let mut revoked = chain_agent("acme/worker", "spiffe://acme.local/ns/prod/sa/worker");
    revoked.lifecycle_state = "revoked".into();
    let records = [
        chain_agent("acme/router", "spiffe://acme.local/ns/prod/sa/router"),
        revoked,
        sample_registry_record(),
    ];
    let (service, audit) = build_counting_service(
        keys,
        CountingRegistry::new(records),
        ChainResolutionMode::Strict,
    );

    let mut claims = bootstrap_claims(keys);
    claims["act_chain"] = serde_json::to_value(inbound_chain_before_append()).unwrap();
    let subject = mint_bootstrap_token(keys, claims);
    let err = service
        .handle(sample_exchange_request(&subject), None)
        .await
        .expect_err("revoked chain entry");

    assert_eq!(err.error, "act_chain_entry_revoked");
    assert!(err.error_description.contains("index 2"));
    assert!(err.error_description.contains("acme/worker"));

    let deny = audit
        .take_events()
        .into_iter()
        .find(|e| e.outcome == "deny")
        .expect("deny audit");
    assert_eq!(deny.decision_reason, "act_chain_entry_revoked");
    assert_eq!(deny.offending_index, Some(2));
    assert_eq!(deny.offending_agent_id.as_deref(), Some("acme/worker"));
}

#[tokio::test]
async fn second_exchange_with_same_chain_uses_chain_cache() {
    let keys = shared_test_keys();
    let records = [
        chain_agent("acme/router", "spiffe://acme.local/ns/prod/sa/router"),
        chain_agent("acme/worker", "spiffe://acme.local/ns/prod/sa/worker"),
        sample_registry_record(),
    ];
    let registry = CountingRegistry::new(records);
    let (service, _) = build_counting_service(keys, registry.clone(), ChainResolutionMode::Cache);

    let mut claims = bootstrap_claims(keys);
    claims["act_chain"] = serde_json::to_value(inbound_chain_before_append()).unwrap();
    let subject = mint_bootstrap_token(keys, claims);
    let request = sample_exchange_request(&subject);

    service.handle(request.clone(), None).await.expect("first");
    let after_first = registry.lookup_count();
    service.handle(request, None).await.expect("second");
    let after_second = registry.lookup_count();

    assert_eq!(
        after_first, after_second,
        "expected chain cache hit on second exchange"
    );
}
