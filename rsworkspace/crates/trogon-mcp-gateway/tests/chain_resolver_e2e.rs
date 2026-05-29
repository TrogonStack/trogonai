use trogon_identity_types::ActChainEntry;
use trogon_mcp_gateway::chain_resolver::{
    ChainResolutionError, ChainResolutionMode, ChainResolver, MockAgentRegistry, RegistryRecord,
};

fn entry(sub: &str, agent_id: &str) -> ActChainEntry {
    ActChainEntry {
        sub: sub.into(),
        agent_id: Some(agent_id.into()),
        wkl: None,
        iat: 1,
    }
}

fn active_record(agent_id: &str) -> RegistryRecord {
    RegistryRecord {
        agent_id: agent_id.to_string(),
        lifecycle_state: "active".into(),
    }
}

fn revoked_record(agent_id: &str) -> RegistryRecord {
    RegistryRecord {
        agent_id: agent_id.to_string(),
        lifecycle_state: "revoked".into(),
    }
}

#[tokio::test]
async fn happy_path_resolves_active_chain() {
    let registry = MockAgentRegistry::new([active_record("acme/oncall-agent")]);
    let resolver = ChainResolver::new(registry);
    let chain = vec![entry("user:alice", "acme/oncall-agent")];

    let resolved = resolver
        .resolve(chain.as_slice(), ChainResolutionMode::Strict)
        .await
        .expect("active agent resolves");

    assert_eq!(resolved.entries, chain);
}

#[tokio::test]
async fn revoked_entry_is_rejected() {
    let registry = MockAgentRegistry::new([revoked_record("acme/revoked-agent")]);
    let resolver = ChainResolver::new(registry);
    let chain = vec![entry("user:alice", "acme/revoked-agent")];

    let err = resolver
        .resolve(chain.as_slice(), ChainResolutionMode::Strict)
        .await
        .expect_err("revoked agent rejected");

    assert_eq!(
        err,
        ChainResolutionError::Revoked {
            index: 0,
            agent_id: "acme/revoked-agent".into(),
        }
    );
}
