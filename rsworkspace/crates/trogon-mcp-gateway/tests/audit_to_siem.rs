//! Integration: JetStream audit publish → SIEM bridge raw passthrough.

use std::time::Duration;

use async_nats::jetstream;
use futures::StreamExt;
use trogon_mcp_gateway::audit::{ensure_audit_stream, publish_audit, AuditEnvelope};
use trogon_mcp_gateway::authz::IdentitySource;
use trogon_mcp_gateway::observability::{
    AuditBridge, ObservabilityConfig, SiemFormat, TRACEPARENT_HEADER,
};
use trogon_nats::{connect, NatsAuth, NatsConfig};
use uuid::Uuid;

async fn require_nats_with_jetstream() -> (async_nats::Client, jetstream::Context) {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let connect_timeout = Duration::from_secs(5);
    let nats_conf = NatsConfig::new(vec![url.clone()], NatsAuth::None);
    let nats = match tokio::time::timeout(connect_timeout, connect(&nats_conf, connect_timeout)).await {
        Ok(Ok(client)) => client,
        Ok(Err(error)) => {
            panic!("NATS test server unavailable at {url}: {error}");
        }
        Err(_) => panic!("NATS test server unavailable at {url}: connect timed out"),
    };

    let jetstream = jetstream::new(nats.clone());
    let probe = format!(
        "trogon.siem.probe.{}",
        Uuid::now_v7().as_simple()
    );
    match tokio::time::timeout(
        Duration::from_secs(5),
        jetstream.get_or_create_stream(jetstream::stream::Config {
            name: format!("SIEM_PROBE_{}", Uuid::now_v7().as_simple()),
            subjects: vec![probe],
            max_messages: 1,
            ..Default::default()
        }),
    )
    .await
    {
        Ok(Ok(_)) => (nats, jetstream),
        Ok(Err(error)) => {
            panic!("NATS test server unavailable at {url}: JetStream error: {error}");
        }
        Err(_) => {
            panic!("NATS test server unavailable at {url}: JetStream stream create timed out");
        }
    }
}

#[tokio::test]
async fn audit_bridge_raw_passthrough_republishes_to_siem_subject() {
    let (nats, jetstream) = require_nats_with_jetstream().await;

    let run_id = Uuid::now_v7().as_simple().to_string();
    let prefix = format!("siem.{run_id}.mcp");
    let stream_name = format!("MCP_AUDIT_SIEM_{run_id}");
    let siem_subject = format!("siem.{run_id}.events");
    let durable = format!("siem-bridge-{run_id}");

    ensure_audit_stream(&jetstream, &stream_name, &prefix)
        .await
        .expect("audit stream");

    let config = ObservabilityConfig {
        otel_endpoint: None,
        otel_service_name: None,
        siem_subject: Some(siem_subject.clone()),
        audit_consumer_durable: durable,
        siem_format: SiemFormat::Raw,
        audit_stream_name: stream_name.clone(),
        mcp_prefix: prefix.clone(),
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let bridge = AuditBridge::start(nats.clone(), config, shutdown_rx);

    let mut siem_sub = nats
        .subscribe(siem_subject.clone())
        .await
        .expect("siem subscribe");

    tokio::time::sleep(Duration::from_millis(250)).await;

    let audit_subject = format!("{prefix}.audit.allow.request.tools");
    let envelope = AuditEnvelope::new(
        format!("{prefix}.gateway.request.fs.tools.call"),
        format!("{prefix}.server.fs.tools.call"),
        "allow",
        "request",
        "tools/call".to_string(),
        Some("acme".to_string()),
        None,
        None,
        IdentitySource::Jwt,
        Some(serde_json::json!("req-siem-1")),
        None,
    );

    publish_audit(
        &jetstream,
        audit_subject.clone(),
        &envelope,
        Duration::from_secs(5),
    )
    .await;

    let received = tokio::time::timeout(Duration::from_secs(10), siem_sub.next())
        .await
        .expect("timed out waiting for SIEM republish")
        .expect("siem subscription ended");

    let payload: serde_json::Value =
        serde_json::from_slice(&received.payload).expect("siem payload json");
    assert_eq!(payload.get("outcome").and_then(|v| v.as_str()), Some("allow"));
    assert_eq!(
        payload.get("jsonrpc_method").and_then(|v| v.as_str()),
        Some("tools/call")
    );
    assert_eq!(
        payload.get("tenant").and_then(|v| v.as_str()),
        Some("acme")
    );

    let traceparent = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
    let mut audit_headers = async_nats::HeaderMap::new();
    audit_headers.insert(TRACEPARENT_HEADER, traceparent);
    jetstream
        .publish_with_headers(
            audit_subject,
            audit_headers,
            serde_json::to_vec(&envelope).expect("serialize").into(),
        )
        .await
        .expect("second audit publish")
        .await
        .expect("second audit ack");

    let received2 = tokio::time::timeout(Duration::from_secs(10), siem_sub.next())
        .await
        .expect("timed out waiting for traced SIEM republish")
        .expect("siem subscription ended");
    assert_eq!(
        received2
            .headers
            .as_ref()
            .and_then(|headers| headers.get("trace_id"))
            .map(|value| value.as_str()),
        Some("0af7651916cd43dd8448eb211c80319c")
    );
    assert_eq!(
        received2
            .headers
            .as_ref()
            .and_then(|headers| headers.get("span_id"))
            .map(|value| value.as_str()),
        Some("b7ad6b7169203331")
    );

    shutdown_tx.send(()).ok();
    tokio::time::timeout(Duration::from_secs(5), bridge)
        .await
        .expect("bridge join timed out")
        .expect("bridge task failed");
}
