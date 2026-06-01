//! Ingress path wires anomaly feature emission after authorization (best-effort).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use mcp_nats::{Config as McpConfig, McpPrefix};
use trogon_mcp_gateway::anomaly::FakeAnomalyEmitter;
use trogon_mcp_gateway::authz::AllowAllPermissionChecker;
use trogon_mcp_gateway::egress::backend_target_aud;
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_nats::{NatsAuth, NatsConfig, connect};
use uuid::Uuid;

mod harness {
    use super::*;

    pub struct GatewayHarness {
        pub prefix: McpPrefix,
        pub gateway_client: Arc<async_nats::Client>,
        pub shutdown_tx: tokio::sync::oneshot::Sender<()>,
        pub join: tokio::task::JoinHandle<Result<(), trogon_mcp_gateway::GatewayError>>,
    }

    pub async fn start_gateway(
        settings: GatewaySettings,
    ) -> GatewayHarness {
        let nats_conf = settings.mcp.nats().clone();
        let prefix = settings.mcp.prefix().clone();

        let backend = connect(&nats_conf, Duration::from_secs(15)).await.expect("backend nats");
        let backend_reply = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"tools": []},
        }))
        .expect("json");

        let subject_out = format!("{}.server.fixture.tools.list", prefix.as_str());
        {
            let mut sub = backend.subscribe(subject_out.clone()).await.expect("sub");
            let backend_nats = backend.clone();
            tokio::spawn(async move {
                while let Some(msg) = sub.next().await {
                    let Some(reply) = msg.reply else { continue };
                    backend_nats
                        .publish_with_headers(reply.to_string(), async_nats::HeaderMap::new(), Bytes::from(backend_reply))
                        .await
                        .ok();
                    backend_nats.flush().await.ok();
                    break;
                }
            });
        }

        let gateway_client = Arc::new(connect(&nats_conf, Duration::from_secs(15)).await.expect("gateway nats"));
        let checker: Arc<dyn trogon_mcp_gateway::authz::PermissionChecker> = Arc::new(AllowAllPermissionChecker);
        let traces = trogon_mcp_gateway::trace::TraceStore::default();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let run_client = gateway_client.clone();
        let join = tokio::spawn(async move {
            trogon_mcp_gateway::run(run_client, checker, traces, settings, async {
                shutdown_rx.await.ok();
            })
            .await
        });

        tokio::time::sleep(Duration::from_millis(400)).await;

        GatewayHarness {
            prefix,
            gateway_client,
            shutdown_tx,
            join,
        }
    }

    pub async fn tools_list_ok(client: &async_nats::Client, prefix: &McpPrefix) {
        let ingress = format!("{}.gateway.request.fixture.tools.list", prefix.as_str());
        let payload = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}});
        let response = client
            .request_with_headers(
                ingress,
                async_nats::HeaderMap::new(),
                serde_json::to_vec(&payload).unwrap().into(),
            )
            .await
            .expect("tools/list");
        let body: serde_json::Value = serde_json::from_slice(&response.payload).expect("json");
        assert_eq!(body.get("id"), Some(&serde_json::json!(1)));
        body.get("result").expect("tools/list success");
    }
}

#[tokio::test]
async fn anomaly_emits_when_emitter_configured() {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let connect = async_nats::connect(url);
    if tokio::time::timeout(Duration::from_secs(2), connect).await.is_err() {
        eprintln!("skip: NATS unavailable (set NATS_URL or start nats-server)");
        return;
    }

    let fake = Arc::new(FakeAnomalyEmitter::new());
    let nats_conf = NatsConfig::new(
        vec![std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into())],
        NatsAuth::None,
    );
    let prefix_segment = format!("{}", Uuid::now_v7().as_simple());
    let prefix_token = format!("{}.mcp", prefix_segment);
    let prefix = McpPrefix::new(&prefix_token).expect("prefix");
    let mcp_conf = McpConfig::new(prefix.clone(), nats_conf).with_operation_timeout(Duration::from_secs(15));

    let settings = GatewaySettings {
        queue_group: format!("q-anomaly-{prefix_segment}"),
        audit_stream_name: "MCP_AUDIT_ANOMALY_EMIT".into(),
        init_audit_stream: false,
        mcp: mcp_conf,
        jwt: trogon_mcp_gateway::jwt::JwtValidator::disabled().expect("jwt off"),
        egress: None,
        chain_resolver: None,
        rate_limit: None,
        anomaly_emitter: Some(fake.clone()),
    };

    let harness = harness::start_gateway(settings).await;
    harness::tools_list_ok(&harness.gateway_client, &harness.prefix).await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let captured = fake.captured_features();
    assert_eq!(captured.len(), 1, "expected one anomaly feature vector");
    let features = &captured[0];
    assert_eq!(features.tenant_id, "unknown");
    assert_eq!(features.agent_id, "anonymous");
    assert_eq!(features.purpose, "");
    assert_eq!(
        features.target,
        backend_target_aud("unknown", "fixture")
    );
    assert_eq!(features.request_id, "1");
    assert_eq!(features.chain_depth, 0);

    harness.shutdown_tx.send(()).ok();
    harness.join.await.expect("join").expect("run");
}

#[tokio::test]
async fn anomaly_emit_failure_does_not_fail_request() {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let connect = async_nats::connect(url);
    if tokio::time::timeout(Duration::from_secs(2), connect).await.is_err() {
        eprintln!("skip: NATS unavailable (set NATS_URL or start nats-server)");
        return;
    }

    let fake = Arc::new(FakeAnomalyEmitter::new());
    fake.set_fail(true);

    let nats_conf = NatsConfig::new(
        vec![std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into())],
        NatsAuth::None,
    );
    let prefix_segment = format!("{}", Uuid::now_v7().as_simple());
    let prefix_token = format!("{}.mcp", prefix_segment);
    let prefix = McpPrefix::new(&prefix_token).expect("prefix");
    let mcp_conf = McpConfig::new(prefix.clone(), nats_conf).with_operation_timeout(Duration::from_secs(15));

    let settings = GatewaySettings {
        queue_group: format!("q-anomaly-fail-{prefix_segment}"),
        audit_stream_name: "MCP_AUDIT_ANOMALY_FAIL".into(),
        init_audit_stream: false,
        mcp: mcp_conf,
        jwt: trogon_mcp_gateway::jwt::JwtValidator::disabled().expect("jwt off"),
        egress: None,
        chain_resolver: None,
        rate_limit: None,
        anomaly_emitter: Some(fake.clone()),
    };

    let harness = harness::start_gateway(settings).await;
    harness::tools_list_ok(&harness.gateway_client, &harness.prefix).await;

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        fake.captured_features().is_empty(),
        "failed emit must not record features"
    );

    harness.shutdown_tx.send(()).ok();
    harness.join.await.expect("join").expect("run");
}
