#![allow(clippy::expect_used, clippy::panic)]

use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use serde_json::json;
use trogon_peer_mcp::{
    AcknowledgePeerBatch, AvailabilityObservation, ConnectorConfigInput, DetachPeer, EnrollPeer, HeartbeatPeer,
    PeerMcp, PreparePeerBatch, ReadPeerMessageStatus, ReadPeerRoster, SendPeerMessage, serve_connector,
};
use trogonai_proto::peers::v1alpha1 as v1;

#[derive(Clone, Debug, PartialEq)]
enum Call {
    Heartbeat(v1::HeartbeatRequest),
    Detach(v1::DetachRequest),
}

struct Backend {
    calls: Arc<Mutex<Vec<Call>>>,
    heartbeat_entered: Arc<tokio::sync::Notify>,
}

impl EnrollPeer for Backend {
    type Error = Infallible;
    async fn enroll(&self, _request: v1::EnrollRequest) -> Result<v1::EnrollResponse, Self::Error> {
        Ok(serde_json::from_value(json!({
            "peerId": "b".repeat(64),
            "attachment": {
                "attachmentRef": {"peerId": "b".repeat(64), "connectorId": "connector_b", "generation": 1},
                "status": "ATTACHMENT_STATUS_ATTACHED", "observedAt": "2026-11-02T12:00:00Z",
                "expiresAt": "2026-11-02T12:00:10Z"
            }
        }))
        .expect("enrollment fixture"))
    }
}
impl PreparePeerBatch for Backend {
    type Error = Infallible;
    async fn prepare_batch(&self, _request: v1::PrepareBatchRequest) -> Result<v1::PrepareBatchResponse, Self::Error> {
        Ok(Default::default())
    }
}
impl ReadPeerRoster for Backend {
    type Error = Infallible;
    async fn roster(&self, _request: v1::RosterRequest) -> Result<v1::RosterResponse, Self::Error> {
        unreachable!("roster is outside the runtime shutdown contract")
    }
}
impl SendPeerMessage for Backend {
    type Error = Infallible;
    async fn send(&self, _request: v1::SendRequest) -> Result<v1::SendResponse, Self::Error> {
        unreachable!("send is outside the runtime shutdown contract")
    }
}
impl ReadPeerMessageStatus for Backend {
    type Error = Infallible;
    async fn status(&self, _request: v1::StatusRequest) -> Result<v1::StatusResponse, Self::Error> {
        unreachable!("status is outside the runtime shutdown contract")
    }
}
impl AcknowledgePeerBatch for Backend {
    type Error = Infallible;
    async fn acknowledge_batch(
        &self,
        _request: v1::AcknowledgeBatchRequest,
    ) -> Result<v1::AcknowledgeBatchResponse, Self::Error> {
        unreachable!("acknowledgment is outside the runtime shutdown contract")
    }
}
impl HeartbeatPeer for Backend {
    type Error = Infallible;
    async fn heartbeat(&self, request: v1::HeartbeatRequest) -> Result<v1::HeartbeatResponse, Self::Error> {
        self.calls.lock().expect("calls").push(Call::Heartbeat(request));
        self.heartbeat_entered.notify_one();
        std::future::pending().await
    }
}
impl DetachPeer for Backend {
    type Error = Infallible;
    async fn detach(&self, request: v1::DetachRequest) -> Result<v1::DetachResponse, Self::Error> {
        self.calls.lock().expect("calls").push(Call::Detach(request));
        Ok(Default::default())
    }
}

async fn terminate_with_inflight_heartbeat(eof: bool) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(tokio::sync::Notify::new());
    let backend = Backend {
        calls: calls.clone(),
        heartbeat_entered: entered.clone(),
    };
    let adapter = Arc::new(
        PeerMcp::new(
            backend,
            ConnectorConfigInput {
                installation_id: "installation_b".into(),
                connector_id: "connector_b".into(),
                collaboration_ref: "collaboration_a".into(),
                display_name: "reviewer".into(),
            },
        )
        .expect("config"),
    );
    let (server_transport, client_transport) = tokio::io::duplex(32 * 1024);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve_connector(
        adapter,
        server_transport,
        AvailabilityObservation::Ready,
        async move {
            let _ = stopped.await;
        },
    ));
    let client = ().serve(client_transport).await.expect("handshake");
    let prompt: CallToolRequestParams = serde_json::from_value(json!({
        "name":"peer_prompt_hook", "arguments":{
            "session_id":"native_b", "prompt_id":"initial", "hook_event_name":"UserPromptSubmit"
        }
    }))
    .expect("prompt request");
    let result = client.call_tool(prompt).await.expect("bind");
    assert_ne!(result.is_error, Some(true));
    entered.notified().await;
    assert!(!server.is_finished());
    let started = tokio::time::Instant::now();
    let cleanup = if eof {
        client.cancel().await.expect("EOF");
        None
    } else {
        stop.send(()).expect("shutdown signal");
        Some(client)
    };
    server
        .await
        .expect("runtime task")
        .expect("heartbeat must release detach lock");
    assert!(started.elapsed() <= Duration::from_secs(1));
    if let Some(client) = cleanup {
        client.cancel().await.expect("client cleanup");
    }
    let recorded = calls.lock().expect("calls");
    assert_eq!(recorded.len(), 2);
    let (Call::Heartbeat(heartbeat), Call::Detach(detach)) = (&recorded[0], &recorded[1]) else {
        panic!("shutdown must detach exactly once after the in-flight heartbeat");
    };
    assert_eq!(heartbeat.attachment_ref, detach.attachment_ref);
}

#[tokio::test(start_paused = true)]
async fn signal_drops_inflight_heartbeat_before_detach_lock_wait() {
    terminate_with_inflight_heartbeat(false).await;
}

#[tokio::test(start_paused = true)]
async fn eof_drops_inflight_heartbeat_before_detach_lock_wait() {
    terminate_with_inflight_heartbeat(true).await;
}
