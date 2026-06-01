//! End-to-end forward path against a live NATS broker (`#[ignored]`).

use bytes::Bytes;
use futures::StreamExt;
use mcp_nats::{Config as McpConfig, McpPrefix};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use trogon_mcp_gateway::authz::AllowAllPermissionChecker;
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_nats::{NatsAuth, NatsConfig, connect};
use uuid::Uuid;

#[tokio::test]
#[ignore = "needs NATS (set NATS_URL, default nats://127.0.0.1:4222). Run: cargo test -p trogon-mcp-gateway -- --ignored"]
async fn gateway_forwards_tools_list_request_reply() {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let connect_timeout = Duration::from_secs(15);
    let nats_conf = NatsConfig::new(vec![url], NatsAuth::None);

    let prefix_segment = format!("{}", Uuid::now_v7().as_simple());
    let prefix_token = format!("{}.mcp", prefix_segment);
    let prefix = McpPrefix::new(&prefix_token).expect("test prefix shape");
    let mcp_conf = McpConfig::new(prefix.clone(), nats_conf.clone()).with_operation_timeout(Duration::from_secs(15));

    let backend_connection = connect(&nats_conf, connect_timeout).await.expect("backend nats");

    let subject_out = format!("{}.server.fixture.tools.list", prefix.as_str());
    let backend_reply = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {"tools": []},
    }))
    .expect("static json");

    {
        let mut subscription = backend_connection
            .subscribe(subject_out.clone())
            .await
            .expect("subscribe backend lane");
        let backend_nats = backend_connection.clone();
        let expected_subject = subject_out.clone();
        tokio::spawn(async move {
            while let Some(msg) = subscription.next().await {
                assert_eq!(msg.subject.as_str(), expected_subject.as_str());
                let Some(reply) = msg.reply.clone() else {
                    continue;
                };
                backend_nats
                    .publish_with_headers(
                        reply.to_string(),
                        async_nats::HeaderMap::new(),
                        Bytes::from(backend_reply),
                    )
                    .await
                    .ok();
                backend_nats.flush().await.ok();
                break;
            }
        });
    }

    let checker: Arc<dyn trogon_mcp_gateway::authz::PermissionChecker> = Arc::new(AllowAllPermissionChecker);
    let traces = trogon_mcp_gateway::trace::TraceStore::default();
    let settings = GatewaySettings {
        queue_group: format!("q-{prefix_segment}"),
        audit_stream_name: "MCP_AUDIT_E2E_IGNORED".into(),
        init_audit_stream: false,
        mcp: mcp_conf,
        jwt: trogon_mcp_gateway::jwt::JwtValidator::disabled().expect("jwt ingress off"),
        egress: None,
        chain_resolver: None,
        rate_limit: None,
        stepup_policy: None,
        stepup_bridge: None,
        freshness_clock: None,
    };

    let gateway_client = Arc::new(connect(&nats_conf, connect_timeout).await.expect("gateway nats"));

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let run_client = gateway_client.clone();
    let join = tokio::spawn(async move {
        trogon_mcp_gateway::run(run_client, checker, traces, settings, async {
            shutdown_rx.await.ok();
        })
        .await
    });

    tokio::time::sleep(Duration::from_millis(350)).await;

    let ingress = format!("{}.gateway.request.fixture.tools.list", prefix.as_str());
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });
    let response = gateway_client
        .request_with_headers(
            ingress.clone(),
            async_nats::HeaderMap::new(),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("client request failed");

    let body: Value = serde_json::from_slice(&response.payload).expect("json-rpc decode");
    assert_eq!(body.get("id"), Some(&serde_json::json!(1)));
    body.get("result").expect("expected tools/list success envelope");

    shutdown_tx.send(()).ok();
    join.await.expect("gateway task panic").expect("gateway run failure");
}
