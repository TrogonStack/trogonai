//! Ingress approval gate wiring: `-32107 approval_required` on pending, forward on grant.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use mcp_nats::{Config as McpConfig, McpPrefix};
use serde_json::Value;
use trogon_mcp_gateway::approvals::{
    ApprovalDecision, ApprovalError, ApprovalGate, ApprovalRequest,
};
use trogon_mcp_gateway::authz::{AllowAllPermissionChecker, AuthzContext, AuthzError, PermissionChecker};
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_mcp_gateway::policy::{MeshGatewayConfig, RiskThresholds};
use trogon_mcp_gateway::rpc_codes;
use trogon_nats::{NatsAuth, NatsConfig, connect};
use uuid::Uuid;

struct StubApprovalGate {
    outcome: ApprovalGateOutcome,
}

enum ApprovalGateOutcome {
    Pending,
    Granted,
}

#[async_trait]
impl ApprovalGate for StubApprovalGate {
    async fn request_approval(
        &self,
        _request_ctx: &ApprovalRequest,
    ) -> Result<ApprovalDecision, ApprovalError> {
        match self.outcome {
            ApprovalGateOutcome::Pending => Err(ApprovalError::Timeout),
            ApprovalGateOutcome::Granted => Ok(ApprovalDecision::Granted {
                approver: "human:test".into(),
                expires_at: i64::MAX,
            }),
        }
    }
}

struct CountingChecker {
    hits: Arc<AtomicUsize>,
}

#[async_trait]
impl PermissionChecker for CountingChecker {
    async fn authorize_mcp_request(&self, _ctx: AuthzContext<'_>) -> Result<bool, AuthzError> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }
}

fn mesh_config_forcing_hitl() -> MeshGatewayConfig {
    MeshGatewayConfig {
        risk: RiskThresholds {
            approval_score: 0,
            deny_score: 10_000,
            step_up_purposes: Vec::new(),
            approval_denials_60s: 10_000,
        },
        ..MeshGatewayConfig::default()
    }
}

struct ApprovalIngressHarness {
    gateway_client: Arc<async_nats::Client>,
    prefix: McpPrefix,
    backend_hit: Arc<AtomicBool>,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
    join: tokio::task::JoinHandle<Result<(), trogon_mcp_gateway::gateway::GatewayError>>,
}

impl ApprovalIngressHarness {
    async fn start(approval_gate: Option<Arc<dyn ApprovalGate>>) -> Self {
        let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
        let connect_timeout = Duration::from_secs(15);
        let nats_conf = NatsConfig::new(vec![url], NatsAuth::None);
        let prefix_segment = format!("{}", Uuid::now_v7().as_simple());
        let prefix_token = format!("{prefix_segment}.mcp");
        let prefix = McpPrefix::new(&prefix_token).expect("test prefix shape");
        let mcp_conf =
            McpConfig::new(prefix.clone(), nats_conf.clone()).with_operation_timeout(Duration::from_secs(15));

        let backend_hit = Arc::new(AtomicBool::new(false));
        let backend_connection = connect(&nats_conf, connect_timeout).await.expect("backend nats");
        let subject_out = format!("{}.server.fixture.tools.call", prefix.as_str());
        let backend_reply = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "result": {"content": []},
        }))
        .expect("static json");
        {
            let mut subscription = backend_connection
                .subscribe(subject_out.clone())
                .await
                .expect("subscribe backend lane");
            let backend_nats = backend_connection.clone();
            let backend_hit = Arc::clone(&backend_hit);
            tokio::spawn(async move {
                while let Some(msg) = subscription.next().await {
                    backend_hit.store(true, Ordering::SeqCst);
                    let Some(reply) = msg.reply.clone() else {
                        continue;
                    };
                    backend_nats
                        .publish_with_headers(
                            reply.to_string(),
                            async_nats::HeaderMap::new(),
                            Bytes::from(backend_reply.clone()),
                        )
                        .await
                        .ok();
                    backend_nats.flush().await.ok();
                    break;
                }
            });
        }

        let authz_hits = Arc::new(AtomicUsize::new(0));
        let checker: Arc<dyn PermissionChecker> = if approval_gate.is_some() {
            Arc::new(CountingChecker {
                hits: Arc::clone(&authz_hits),
            })
        } else {
            Arc::new(AllowAllPermissionChecker)
        };

        let settings = GatewaySettings {
            queue_group: format!("q-{prefix_segment}"),
            audit_stream_name: "MCP_AUDIT_APPROVAL_INGRESS".into(),
            init_audit_stream: false,
            mcp: mcp_conf,
            jwt: trogon_mcp_gateway::jwt::JwtValidator::disabled().expect("jwt ingress off"),
            egress: None,
            chain_resolver: None,
            rate_limit: None,
            approval_gate,
            mesh_config: mesh_config_forcing_hitl(),
        };

        let gateway_client = Arc::new(connect(&nats_conf, connect_timeout).await.expect("gateway nats"));
        let traces = trogon_mcp_gateway::trace::TraceStore::default();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let run_client = gateway_client.clone();
        let join = tokio::spawn(async move {
            trogon_mcp_gateway::run(run_client, checker, traces, settings, async {
                shutdown_rx.await.ok();
            })
            .await
        });

        tokio::time::sleep(Duration::from_millis(350)).await;

        Self {
            gateway_client,
            prefix,
            backend_hit,
            shutdown_tx,
            join,
        }
    }

    async fn tools_call(&self) -> Value {
        let ingress = format!("{}.gateway.request.fixture.tools.call", self.prefix.as_str());
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "tools/call",
            "params": {"name": "deploy", "arguments": {"env": "prod"}}
        });
        let response = self
            .gateway_client
            .request_with_headers(
                ingress,
                async_nats::HeaderMap::new(),
                serde_json::to_vec(&payload).unwrap().into(),
            )
            .await
            .expect("client request failed");
        serde_json::from_slice(&response.payload).expect("json-rpc decode")
    }

    async fn shutdown(self) {
        self.shutdown_tx.send(()).ok();
        self.join.await.expect("gateway task panic").expect("gateway run failure");
    }
}

#[tokio::test]
async fn pending_gate_emits_approval_required_envelope() {
    let harness = ApprovalIngressHarness::start(Some(Arc::new(StubApprovalGate {
        outcome: ApprovalGateOutcome::Pending,
    })))
    .await;

    let body = harness.tools_call().await;
    assert_eq!(body.get("id"), Some(&serde_json::json!(42)));
    assert_eq!(
        body["error"]["code"].as_i64(),
        Some(i64::from(rpc_codes::APPROVAL_REQUIRED))
    );
    assert_eq!(body["error"]["message"], "approval_required");
    assert!(body["error"]["data"]["approval_subject"]
        .as_str()
        .unwrap()
        .starts_with("mcp.approvals."));
    assert_eq!(body["error"]["data"]["request_id"], "42");
    assert!(!harness.backend_hit.load(Ordering::SeqCst), "backend must not be called");

    harness.shutdown().await;
}

#[tokio::test]
async fn granted_gate_forwards_to_backend_and_authz() {
    let harness = ApprovalIngressHarness::start(Some(Arc::new(StubApprovalGate {
        outcome: ApprovalGateOutcome::Granted,
    })))
    .await;

    let body = harness.tools_call().await;
    assert_eq!(body.get("id"), Some(&serde_json::json!(42)));
    body.get("result").expect("expected tools/call success envelope");
    assert!(harness.backend_hit.load(Ordering::SeqCst), "backend should receive forwarded call");

    harness.shutdown().await;
}

#[tokio::test]
async fn absent_gate_skips_hitl_even_when_risk_would_require_approval() {
    let harness = ApprovalIngressHarness::start(None).await;

    let body = harness.tools_call().await;
    assert_eq!(body.get("id"), Some(&serde_json::json!(42)));
    body.get("result").expect("expected tools/call success envelope");
    assert!(harness.backend_hit.load(Ordering::SeqCst));

    harness.shutdown().await;
}
