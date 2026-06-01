//! Step-up policy integration at the gateway ingress hook.

use std::sync::Arc;

use async_trait::async_trait;
use mcp_nats::{Config as McpConfig, McpPrefix};
use serde_json::json;
use std::sync::Mutex;
use trogon_mcp_gateway::gateway::{GatewaySettings, StepUpIngressBlock, evaluate_step_up};
use trogon_mcp_gateway::jwt::JwtValidator;
use trogon_mcp_gateway::rpc_codes;
use trogon_mcp_gateway::schema_cache::{SchemaCacheConfig, SchemaCacheRuntime, ServerId, sniff_tools_list_reply};
use trogon_mcp_gateway::stepup::{
    ApprovalBridge, FreshnessClock, StepUpPolicy, StepUpRequestCtx, TestFreshnessClock,
};
use trogon_nats::{NatsAuth, NatsConfig};

struct RecordingBridge {
    escalated: Arc<Mutex<Vec<StepUpRequestCtx>>>,
}

impl RecordingBridge {
    fn new() -> Self {
        Self {
            escalated: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl ApprovalBridge for RecordingBridge {
    async fn escalate(&self, request_ctx: &StepUpRequestCtx) -> Result<(), trogon_mcp_gateway::stepup::StepUpError> {
        self.escalated.lock().expect("lock").push(request_ctx.clone());
        Ok(())
    }
}

fn base_settings(
    stepup_policy: Option<Arc<StepUpPolicy>>,
    freshness_clock: Option<Arc<dyn FreshnessClock>>,
    stepup_bridge: Option<Arc<dyn ApprovalBridge>>,
) -> GatewaySettings {
    let prefix = McpPrefix::new("stepup.test.mcp").expect("prefix");
    let mcp = McpConfig::new(prefix, NatsConfig::new(vec!["nats://127.0.0.1:4222".into()], NatsAuth::None));
    GatewaySettings {
        queue_group: "stepup-test".into(),
        audit_stream_name: "MCP_AUDIT_STEPUP".into(),
        init_audit_stream: false,
        mcp,
        jwt: JwtValidator::disabled().expect("jwt off"),
        egress: None,
        chain_resolver: None,
        rate_limit: None,
        approval_gate: None,
        mesh_config: trogon_mcp_gateway::policy::MeshGatewayConfig::default(),
        context_throttle: None,
        anomaly_emitter: None,
        stepup_policy,
        stepup_bridge,
        freshness_clock,
    }
}

async fn install_runtime() -> Arc<SchemaCacheRuntime> {
    if let Some(runtime) = SchemaCacheRuntime::shared() {
        return runtime;
    }
    let runtime = SchemaCacheRuntime::new(SchemaCacheConfig::default());
    let _ = SchemaCacheRuntime::install(runtime.clone());
    SchemaCacheRuntime::shared().expect("runtime installed")
}

async fn seed_tool_annotations(server_id: &str, tool_name: &str, sensitive: bool) {
    let runtime = install_runtime().await;
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "tools": [{
                "name": tool_name,
                "annotations": { "sensitive": sensitive },
                "inputSchema": { "type": "object" }
            }]
        }
    });
    sniff_tools_list_reply(
        &runtime.cache,
        &runtime.config,
        &ServerId::new(server_id),
        &serde_json::to_vec(&payload).unwrap(),
    )
    .await
    .expect("sniff");
}

fn jwt_claims(auth_method: &str, auth_time: Option<i64>) -> trogon_mcp_gateway::jwt::VerifiedJwtClaims {
    trogon_mcp_gateway::jwt::VerifiedJwtClaims {
        auth_method: Some(auth_method.to_string()),
        auth_time,
        ..Default::default()
    }
}

#[tokio::test]
async fn stepup_allow_when_sensitive_false() {
    seed_tool_annotations("fixture_allow", "safe_tool", false).await;
    let settings = base_settings(
        Some(Arc::new(StepUpPolicy::default())),
        Some(Arc::new(TestFreshnessClock(1_000))),
        None,
    );
    let block = evaluate_step_up(
        &settings,
        "tools/call",
        Some("safe_tool"),
        &ServerId::new("fixture_allow"),
        &Some(json!(42)),
        &jwt_claims("oidc", Some(900)),
    )
    .await;
    assert!(block.is_none(), "non-sensitive tools should pass step-up gate");
}

#[tokio::test]
async fn stepup_reauth_when_auth_too_old() {
    seed_tool_annotations("fixture_reauth", "danger_tool", true).await;
    let settings = base_settings(
        Some(Arc::new(StepUpPolicy::default())),
        Some(Arc::new(TestFreshnessClock(1_000))),
        None,
    );
    let block = evaluate_step_up(
        &settings,
        "tools/call",
        Some("danger_tool"),
        &ServerId::new("fixture_reauth"),
        &Some(json!("req-stale")),
        &jwt_claims("oidc", Some(600)),
    )
    .await;
    match block {
        Some(StepUpIngressBlock::Reauth { max_age_seconds }) => {
            assert_eq!(max_age_seconds, 300);
        }
        other => panic!("expected reauth block, got {other:?}"),
    }
}

#[tokio::test]
async fn stepup_approval_routes_to_approval_envelope() {
    seed_tool_annotations("fixture_approval", "danger_tool", true).await;
    let bridge = Arc::new(RecordingBridge::new());
    let settings = base_settings(
        Some(Arc::new(StepUpPolicy::default())),
        Some(Arc::new(TestFreshnessClock(1_000))),
        Some(bridge.clone()),
    );
    let block = evaluate_step_up(
        &settings,
        "tools/call",
        Some("danger_tool"),
        &ServerId::new("fixture_approval"),
        &Some(json!("req-approval")),
        &jwt_claims("workload-ci", Some(900)),
    )
    .await;
    let Some(StepUpIngressBlock::Approval { body }) = block else {
        panic!("expected approval envelope, got {block:?}");
    };
    assert_eq!(bridge.escalated.lock().expect("lock").len(), 1);
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(parsed["error"]["code"], rpc_codes::APPROVAL_REQUIRED);
    assert_eq!(parsed["error"]["message"], "approval_required");
    assert_eq!(
        parsed["error"]["data"]["approval_subject"],
        "mcp.approvals.step-up.req-approval"
    );
    assert_eq!(
        parsed["error"]["data"]["reason"],
        "sensitive_tool_requires_service_approval"
    );
}
