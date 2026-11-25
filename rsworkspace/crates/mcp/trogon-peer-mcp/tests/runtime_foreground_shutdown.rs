#![allow(clippy::expect_used, clippy::panic)]

use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use serde_json::json;
use trogon_peer_mcp::{
    AcknowledgePeerBatch, AvailabilityObservation, ConnectorConfigInput, DetachPeer, EnrollPeer, HeartbeatPeer,
    PeerMcp, PeerMcpError, PreparePeerBatch, ReadPeerMessageStatus, ReadPeerRoster, SendPeerMessage, serve_connector,
};
use trogonai_proto::peers::v1alpha1 as v1;

#[derive(Clone, Debug, PartialEq)]
enum Call {
    Roster(v1::RosterRequest),
    Detach(v1::DetachRequest),
}

struct Backend {
    calls: Arc<Mutex<Vec<Call>>>,
    entered: Arc<tokio::sync::Notify>,
    dropped: Arc<AtomicUsize>,
}

impl EnrollPeer for Backend {
    type Error = Infallible;
    async fn enroll(&self, _request: v1::EnrollRequest) -> Result<v1::EnrollResponse, Self::Error> {
        Ok(serde_json::from_value(json!({
            "peerId": "b".repeat(64),
            "attachment": {
                "attachmentRef": {"peerId": "b".repeat(64), "connectorId": "connector_b", "generation": 7},
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
    async fn roster(&self, request: v1::RosterRequest) -> Result<v1::RosterResponse, Self::Error> {
        let _drop = DropProbe(self.dropped.clone());
        self.calls.lock().expect("calls").push(Call::Roster(request));
        self.entered.notify_one();
        std::future::pending().await
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
    async fn heartbeat(&self, _request: v1::HeartbeatRequest) -> Result<v1::HeartbeatResponse, Self::Error> {
        unreachable!("shutdown must finish before the next heartbeat")
    }
}
impl DetachPeer for Backend {
    type Error = Infallible;
    async fn detach(&self, request: v1::DetachRequest) -> Result<v1::DetachResponse, Self::Error> {
        assert_eq!(
            self.dropped.load(Ordering::SeqCst),
            1,
            "detach must follow foreground cancellation"
        );
        self.calls.lock().expect("calls").push(Call::Detach(request));
        Ok(Default::default())
    }
}

struct DropProbe(Arc<AtomicUsize>);
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy)]
enum Termination {
    Signal,
    Eof,
}

fn roster() -> CallToolRequestParams {
    serde_json::from_value(json!({"name":"peer_roster", "arguments":{}})).expect("roster request")
}

async fn terminate_with_blocked_model_tool(termination: Termination) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(AtomicUsize::new(0));
    let adapter = Arc::new(
        PeerMcp::new(
            Backend {
                calls: calls.clone(),
                entered: entered.clone(),
                dropped: dropped.clone(),
            },
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
        adapter.clone(),
        server_transport,
        AvailabilityObservation::Ready,
        async move {
            let _ = stopped.await;
        },
    ));
    let client = ().serve(client_transport).await.expect("handshake");
    let prompt = serde_json::from_value(json!({
        "name":"peer_prompt_hook", "arguments":{
            "session_id":"native_b", "prompt_id":"initial", "hook_event_name":"UserPromptSubmit"
        }
    }))
    .expect("prompt request");
    assert_ne!(client.call_tool(prompt).await.expect("bind").is_error, Some(true));
    let peer = client.peer().clone();
    let work = tokio::spawn(async move { peer.call_tool(roster()).await });
    entered.notified().await;
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    assert!(!server.is_finished());
    let started = tokio::time::Instant::now();
    let cleanup = match termination {
        Termination::Eof => {
            client.cancel().await.expect("EOF");
            None
        }
        Termination::Signal => {
            stop.send(()).expect("shutdown signal");
            Some(client)
        }
    };
    let completed = tokio::time::timeout(Duration::from_secs(1), server).await;
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        1,
        "shutdown must drop the blocked model backend future within the existing cleanup budget"
    );
    completed
        .expect("runtime cleanup budget")
        .expect("runtime task")
        .expect("foreground cancellation must release detach lock");
    assert!(started.elapsed() <= Duration::from_secs(1));
    if let Some(client) = cleanup {
        client.cancel().await.expect("client cleanup");
    }
    let _outcome = work.await.expect("model task");
    assert!(matches!(adapter.invoke(roster()).await, Err(PeerMcpError::Detached)));
    let recorded = calls.lock().expect("calls");
    assert_eq!(recorded.len(), 2, "no model work may restart after shutdown");
    let (Call::Roster(roster), Call::Detach(detach)) = (&recorded[0], &recorded[1]) else {
        panic!("shutdown must detach exactly once after the blocked model call");
    };
    assert_eq!(roster.attachment_ref, detach.attachment_ref);
    let attachment = detach.attachment_ref.as_option().expect("validated attachment");
    assert_eq!(attachment.peer_id.as_deref(), Some("b".repeat(64).as_str()));
    assert_eq!(attachment.connector_id.as_deref(), Some("connector_b"));
    assert_eq!(attachment.generation, Some(7));
}

#[tokio::test(start_paused = true)]
async fn signal_cancels_blocked_model_tool_before_detach() {
    terminate_with_blocked_model_tool(Termination::Signal).await;
}

#[tokio::test(start_paused = true)]
async fn eof_cancels_blocked_model_tool_before_sdk_response_drain() {
    terminate_with_blocked_model_tool(Termination::Eof).await;
}
