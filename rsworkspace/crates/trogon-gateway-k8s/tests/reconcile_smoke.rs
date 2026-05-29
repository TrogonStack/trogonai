//! Smoke test: one reconcile pass with in-memory KV (no cluster required).

use std::sync::Arc;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use trogon_gateway_k8s::controller::policy::publish_bytes;
use trogon_gateway_k8s::crd::{MCPGatewayConfig, MCPGatewayConfigSpec};
use trogon_gateway_k8s::nats::MemoryConfigKv;
use trogon_gateway_k8s::projection::{decode_gateway_config, project_mcp_gateway_config};

#[tokio::test]
async fn reconcile_writes_mcp_gateway_config_to_memory_kv() {
    let memory = MemoryConfigKv::default();
    let kv: Arc<dyn trogon_gateway_k8s::nats::ConfigKv> = Arc::new(memory.clone());
    let resource = MCPGatewayConfig {
        metadata: ObjectMeta {
            name: Some("edge".to_string()),
            namespace: Some("tenant-acme".to_string()),
            uid: Some("uid-smoke".to_string()),
            generation: Some(1),
            ..Default::default()
        },
        spec: MCPGatewayConfigSpec {
            bundle_ref: "acme/github-smoke".to_string(),
            tenant_id: "acme".to_string(),
            redaction_policies: vec!["pii-default".to_string()],
            rate_limits: [("caller".to_string(), 50)].into(),
        },
        status: None,
    };

    let projection = project_mcp_gateway_config(&resource).expect("project");
    let bytes = trogon_gateway_k8s::projection::encode_gateway_config(&projection.config)
        .expect("encode");
    publish_bytes(&kv, &projection.key, &bytes, &projection.content_hash)
        .await
        .expect("kv put");

    let stored = memory.get(&projection.key).await.expect("stored value");
    let decoded = decode_gateway_config(&stored).expect("decode");
    assert_eq!(decoded.tenant_id, "acme");
    assert_eq!(decoded.bundle_ref, "acme/github-smoke");
}
