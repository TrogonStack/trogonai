use trogon_identity_types::ActChainEntry;
use trogon_mcp_gateway::ingress::{IngressChainResolver, MeshGatewayConfig};
use trogon_mcp_gateway::rpc_codes;
use trogon_sts::cache::RegistryCache;
use trogon_sts::chain_resolution::ChainResolutionMode;
use trogon_sts::registry::{AgentRegistryRecord, InMemoryRegistry};

fn sample_record() -> AgentRegistryRecord {
    AgentRegistryRecord {
        agent_id: "acme/oncall-agent".into(),
        agent_version: "1.0.0".into(),
        agent_definition_digest: "sha256:abc".into(),
        owner_team: "platform".into(),
        allowed_workloads: vec!["spiffe://acme.local/ns/prod/sa/oncall-agent".into()],
        allowed_tools: vec![],
        allowed_audiences: vec![],
        allowed_purposes: vec![],
        mesh_token_ttl_s: None,
        metadata: None,
        lifecycle_state: "active".into(),
    }
}

fn chain_with_agent() -> Vec<ActChainEntry> {
    vec![ActChainEntry {
        sub: "user:alice".into(),
        agent_id: Some("acme/oncall-agent".into()),
        wkl: Some("spiffe://acme.local/ns/prod/sa/oncall-agent".into()),
        iat: 1,
    }]
}

#[tokio::test]
async fn strict_rejects_unknown_wkl() {
    let registry = RegistryCache::new(InMemoryRegistry::new([sample_record()]));
    let resolver = IngressChainResolver::new(registry, ChainResolutionMode::Strict);
    let chain = vec![ActChainEntry {
        sub: "user:alice".into(),
        agent_id: Some("acme/oncall-agent".into()),
        wkl: Some("spiffe://acme.local/ns/prod/sa/other".into()),
        iat: 1,
    }];
    let deny = resolver
        .resolve_inbound_chain(Some(chain.as_slice()))
        .await
        .expect("deny");
    assert_eq!(deny.code, rpc_codes::ACT_CHAIN_UNRESOLVED);
    assert_eq!(deny.message, "act_chain_unresolved");
}

#[tokio::test]
async fn cache_mode_logs_but_allows_unknown_wkl() {
    let registry = RegistryCache::new(InMemoryRegistry::new([sample_record()]));
    let resolver = IngressChainResolver::new(registry, ChainResolutionMode::Cache);
    let chain = vec![ActChainEntry {
        sub: "user:alice".into(),
        agent_id: Some("acme/oncall-agent".into()),
        wkl: Some("spiffe://acme.local/ns/prod/sa/other".into()),
        iat: 1,
    }];
    assert!(
        resolver
            .resolve_inbound_chain(Some(chain.as_slice()))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn strict_allows_known_wkl() {
    let registry = RegistryCache::new(InMemoryRegistry::new([sample_record()]));
    let resolver = IngressChainResolver::new(registry, ChainResolutionMode::Strict);
    assert!(
        resolver
            .resolve_inbound_chain(Some(chain_with_agent().as_slice()))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn human_wkl_skips_registry_lookup() {
    let registry = RegistryCache::new(InMemoryRegistry::new([sample_record()]));
    let resolver = IngressChainResolver::new(registry, ChainResolutionMode::Strict);
    let chain = vec![ActChainEntry {
        sub: "user:alice".into(),
        agent_id: None,
        wkl: Some("human".into()),
        iat: 1,
    }];
    assert!(
        resolver
            .resolve_inbound_chain(Some(chain.as_slice()))
            .await
            .is_none()
    );
}

#[test]
fn mesh_gateway_config_parses_chain_mode() {
    struct Env(std::collections::HashMap<String, String>);
    impl trogon_std::env::ReadEnv for Env {
        fn var(&self, key: &str) -> Result<String, std::env::VarError> {
            self.0
                .get(key)
                .cloned()
                .ok_or(std::env::VarError::NotPresent)
        }
    }
    let mut map = std::collections::HashMap::new();
    map.insert(MeshGatewayConfig::ENV_CHAIN_RESOLUTION_MODE.into(), "strict".into());
    let cfg = MeshGatewayConfig::from_env(&Env(map));
    assert_eq!(cfg.chain_resolution_mode, ChainResolutionMode::Strict);
}
