#![allow(clippy::expect_used, clippy::panic)]

use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::{ServerHandler, ServiceExt};
use serde_json::{Value, json};
use trogon_peer_mcp::{
    AcknowledgePeerBatch, ConnectorConfigInput, EnrollPeer, PeerMcp, PeerMcpError, PreparePeerBatch,
    ReadPeerMessageStatus, ReadPeerRoster, SendPeerMessage,
};
use trogonai_proto::peers::v1alpha1 as v1;

#[derive(Clone, Debug, PartialEq)]
enum ObservedCall {
    Enroll(v1::EnrollRequest),
    Prepare(v1::PrepareBatchRequest),
    Roster(v1::RosterRequest),
    Send(v1::SendRequest),
    Status(v1::StatusRequest),
    Acknowledge(v1::AcknowledgeBatchRequest),
}

#[derive(Debug, thiserror::Error)]
#[error("batch is not assigned to this test recipient")]
struct UnknownBatchError;

#[derive(Clone)]
struct Backend {
    calls: Arc<Mutex<Vec<ObservedCall>>>,
    acknowledged: Arc<AtomicBool>,
    roster_response: v1::RosterResponse,
    send_response: v1::SendResponse,
    status_response: v1::StatusResponse,
    acknowledge_response: v1::AcknowledgeBatchResponse,
}

fn local_peer() -> String {
    "b".repeat(64)
}
fn remote_peer() -> String {
    "a".repeat(64)
}

fn attachment() -> v1::AttachmentRef {
    v1::AttachmentRef {
        peer_id: Some(local_peer()),
        connector_id: Some("connector_b".into()),
        generation: Some(1),
    }
}

fn config() -> ConnectorConfigInput {
    ConnectorConfigInput {
        installation_id: "installation_b".into(),
        connector_id: "connector_b".into(),
        collaboration_ref: "collaboration_a".into(),
        display_name: "reviewer".into(),
    }
}

fn metadata() -> v1::MessageMetadata {
    serde_json::from_value(json!({
        "messageId":"outgoing_1","senderPeerId":local_peer(),"recipientPeerId":remote_peer(),
        "collaborationRef":"collaboration_a","contentRef":"internal_blob_ref","contentSizeBytes":13,
        "acceptedAt":"2026-11-03T12:00:00Z","expiresAt":"2026-11-03T13:00:00Z"
    }))
    .expect("generated accepted message fixture")
}

fn backend() -> Backend {
    Backend {
        calls: Arc::new(Mutex::new(Vec::new())),
        acknowledged: Arc::new(AtomicBool::new(false)),
        roster_response: serde_json::from_value(json!({
            "peers":[{"peerId":remote_peer(),"displayName":"builder","availability":"PEER_AVAILABILITY_READY",
                "deliveryCapability":"DELIVERY_CAPABILITY_ON_USER_PROMPT",
                "observedAt":"2026-11-03T12:00:00Z","expiresAt":"2026-11-03T12:00:10Z"}],
            "nextPageToken":"next_cursor"
        }))
        .expect("generated roster fixture"),
        send_response: v1::SendResponse {
            message: metadata().into(),
        },
        status_response: serde_json::from_value(json!({
            "message":metadata(), "status":"MESSAGE_STATUS_BATCH_PREPARED", "observedAt":"2026-11-03T12:00:01Z"
        }))
        .expect("generated status fixture"),
        acknowledge_response: serde_json::from_value(json!({"batch":{
            "batchId":"batch_1","messageIds":["incoming_1"],"preparedAt":"2026-11-03T12:00:00Z",
            "acknowledgedAt":"2026-11-03T12:00:01Z"
        }}))
        .expect("generated acknowledgment fixture"),
    }
}

impl EnrollPeer for Backend {
    type Error = Infallible;

    async fn enroll(&self, request: v1::EnrollRequest) -> Result<v1::EnrollResponse, Self::Error> {
        self.calls.lock().expect("recorder").push(ObservedCall::Enroll(request));
        Ok(serde_json::from_value(json!({
            "peerId":local_peer(), "attachment":{
                "attachment":attachment(),"status":"ATTACHMENT_STATUS_ATTACHED",
                "observedAt":"2026-11-03T12:00:00Z","expiresAt":"2026-11-03T12:00:10Z"
            }
        }))
        .expect("generated enrollment fixture"))
    }
}

impl PreparePeerBatch for Backend {
    type Error = Infallible;

    async fn prepare_batch(&self, request: v1::PrepareBatchRequest) -> Result<v1::PrepareBatchResponse, Self::Error> {
        self.calls
            .lock()
            .expect("recorder")
            .push(ObservedCall::Prepare(request));
        if self.acknowledged.load(Ordering::SeqCst) {
            return Ok(Default::default());
        }
        Ok(serde_json::from_value(json!({
            "batch":{"batchId":"batch_1","messageIds":["incoming_1"],"preparedAt":"2026-11-03T12:00:00Z"},
            "messages":[{"metadata":{
                "messageId":"incoming_1","senderPeerId":remote_peer(),"recipientPeerId":local_peer(),
                "collaborationRef":"collaboration_a","contentRef":"incoming_blob_ref","contentSizeBytes":14,
                "acceptedAt":"2026-11-03T11:59:00Z","expiresAt":"2026-11-03T12:59:00Z"
            },"text":"body from peer"}],
            "contentAvailability":"CONTENT_AVAILABILITY_AVAILABLE"
        }))
        .expect("generated preparation fixture"))
    }
}

impl ReadPeerRoster for Backend {
    type Error = Infallible;

    async fn roster(&self, request: v1::RosterRequest) -> Result<v1::RosterResponse, Self::Error> {
        self.calls.lock().expect("recorder").push(ObservedCall::Roster(request));
        Ok(self.roster_response.clone())
    }
}

impl SendPeerMessage for Backend {
    type Error = Infallible;

    async fn send(&self, request: v1::SendRequest) -> Result<v1::SendResponse, Self::Error> {
        self.calls.lock().expect("recorder").push(ObservedCall::Send(request));
        Ok(self.send_response.clone())
    }
}

impl ReadPeerMessageStatus for Backend {
    type Error = Infallible;

    async fn status(&self, request: v1::StatusRequest) -> Result<v1::StatusResponse, Self::Error> {
        self.calls.lock().expect("recorder").push(ObservedCall::Status(request));
        Ok(self.status_response.clone())
    }
}

impl AcknowledgePeerBatch for Backend {
    type Error = UnknownBatchError;

    async fn acknowledge_batch(
        &self,
        request: v1::AcknowledgeBatchRequest,
    ) -> Result<v1::AcknowledgeBatchResponse, Self::Error> {
        let known = request.batch_id.as_deref() == Some("batch_1");
        self.calls
            .lock()
            .expect("recorder")
            .push(ObservedCall::Acknowledge(request));
        if !known {
            return Err(UnknownBatchError);
        }
        self.acknowledged.store(true, Ordering::SeqCst);
        Ok(self.acknowledge_response.clone())
    }
}

fn request(name: &str, arguments: Value) -> CallToolRequestParams {
    serde_json::from_value(json!({"name":name,"arguments":arguments})).expect("MCP tool request")
}

fn prompt() -> CallToolRequestParams {
    request(
        "peer_prompt_hook",
        json!({"session_id":"native_b","prompt_id":"prompt_1","hook_event_name":"UserPromptSubmit"}),
    )
}

async fn bind(adapter: &PeerMcp<Backend>) {
    adapter.invoke(prompt()).await.expect("native hook binds connector");
}

fn result_json(result: CallToolResult) -> Value {
    let value = serde_json::to_value(result).expect("MCP result");
    assert_ne!(value["isError"], true);
    assert_eq!(value["content"].as_array().expect("content").len(), 1);
    serde_json::from_str(value["content"][0]["text"].as_str().expect("text result")).expect("JSON metadata")
}

fn assert_metadata_only(value: &Value) {
    let encoded = serde_json::to_string(value).expect("metadata JSON");
    for forbidden in [
        "body from peer",
        "outbound body",
        "internal_blob_ref",
        "incoming_blob_ref",
        "hookSpecificOutput",
        "additionalContext",
        "contentRef",
        "contentDigest",
    ] {
        assert!(!encoded.contains(forbidden), "metadata exposed {forbidden}");
    }
}

fn model_calls() -> Vec<(&'static str, Value)> {
    vec![
        ("peer_roster", json!({"page_size":2,"page_token":"cursor_1"})),
        (
            "peer_send",
            json!({"recipient_peer_id":remote_peer(),"idempotency_key":"send_1","text":"outbound body","reply_to_message_id":"incoming_1"}),
        ),
        ("peer_status", json!({"idempotency_key":"send_1"})),
        ("peer_acknowledge_batch", json!({"batch_id":"batch_1"})),
    ]
}

#[tokio::test]
async fn roster_uses_the_bound_attachment_and_returns_only_authorized_metadata() {
    let backend = backend();
    let calls = backend.calls.clone();
    let adapter = PeerMcp::new(backend, config()).expect("valid config");
    bind(&adapter).await;
    let output = result_json(
        adapter
            .invoke(request("peer_roster", json!({"page_size":2,"page_token":"cursor_1"})))
            .await
            .expect("roster"),
    );
    assert_eq!(output["peers"][0]["peerId"], remote_peer());
    assert_eq!(output["nextPageToken"], "next_cursor");
    assert_metadata_only(&output);
    let recorded = calls.lock().expect("recorder");
    assert_eq!(recorded.len(), 3);
    let ObservedCall::Roster(request) = &recorded[2] else {
        panic!("expected roster request")
    };
    assert_eq!(request.attachment.as_option(), Some(&attachment()));
    assert_eq!(request.page_size, Some(2));
    assert_eq!(request.page_token.as_deref(), Some("cursor_1"));
}

#[tokio::test]
async fn send_and_status_preserve_the_key_and_binding_without_echoing_bodies() {
    let backend = backend();
    let calls = backend.calls.clone();
    let adapter = PeerMcp::new(backend, config()).expect("valid config");
    bind(&adapter).await;
    let arguments = model_calls().remove(1).1;
    let sent = result_json(
        adapter
            .invoke(request("peer_send", arguments.clone()))
            .await
            .expect("accepted send"),
    );
    let retried = result_json(
        adapter
            .invoke(request("peer_send", arguments))
            .await
            .expect("exact caller retry"),
    );
    assert_eq!(sent, retried);
    assert_eq!(sent["message_id"], "outgoing_1");
    assert_eq!(sent["sender_peer_id"], local_peer());
    assert_eq!(sent["recipient_peer_id"], remote_peer());
    assert_metadata_only(&sent);
    let status = result_json(
        adapter
            .invoke(request("peer_status", json!({"idempotency_key":"send_1"})))
            .await
            .expect("status"),
    );
    assert_eq!(status["message_id"], "outgoing_1");
    assert_eq!(status["status"], "MESSAGE_STATUS_BATCH_PREPARED");
    assert_metadata_only(&status);
    let recorded = calls.lock().expect("recorder");
    assert_eq!(recorded.len(), 5);
    let ObservedCall::Send(first) = &recorded[2] else {
        panic!("expected send request")
    };
    let ObservedCall::Send(second) = &recorded[3] else {
        panic!("expected retry request")
    };
    assert_eq!(first, second);
    assert_eq!(first.attachment.as_option(), Some(&attachment()));
    assert_eq!(first.text.as_deref(), Some("outbound body"));
    assert_eq!(first.idempotency_key.as_deref(), Some("send_1"));
    assert_eq!(first.reply_to_message_id.as_deref(), Some("incoming_1"));
    let ObservedCall::Status(status) = &recorded[4] else {
        panic!("expected status request")
    };
    assert_eq!(status.attachment.as_option(), Some(&attachment()));
    assert_eq!(status.idempotency_key.as_deref(), Some("send_1"));
}

#[tokio::test]
async fn application_acknowledgment_is_explicit_recipient_scoped_and_stops_backend_replay() {
    let backend = backend();
    let calls = backend.calls.clone();
    let acknowledged = backend.acknowledged.clone();
    let adapter = PeerMcp::new(backend, config()).expect("valid config");
    bind(&adapter).await;
    assert!(!acknowledged.load(Ordering::SeqCst));
    let ack = request("peer_acknowledge_batch", json!({"batch_id":"batch_1"}));
    let first = result_json(adapter.invoke(ack.clone()).await.expect("application acknowledgment"));
    let retried = result_json(
        adapter
            .invoke(ack)
            .await
            .expect("idempotent application acknowledgment"),
    );
    assert_eq!(first, retried);
    assert_eq!(first["batch_id"], "batch_1");
    assert_eq!(first["acknowledgment"], "APPLICATION_ACKNOWLEDGED");
    assert_metadata_only(&first);
    assert!(acknowledged.load(Ordering::SeqCst));
    let next = result_json(adapter.invoke(prompt()).await.expect("next prompt"));
    assert_eq!(next["hookSpecificOutput"]["additionalContext"], "");
    let recorded = calls.lock().expect("recorder");
    let ObservedCall::Acknowledge(request) = &recorded[2] else {
        panic!("expected acknowledgment request")
    };
    assert_eq!(request.attachment.as_option(), Some(&attachment()));
    assert_eq!(request.batch_id.as_deref(), Some("batch_1"));
}

#[tokio::test]
async fn model_operations_cannot_run_before_native_hook_binding() {
    let backend = backend();
    let calls = backend.calls.clone();
    let adapter = PeerMcp::new(backend, config()).expect("valid config");
    for (tool, arguments) in model_calls() {
        assert!(matches!(
            adapter.invoke(request(tool, arguments)).await,
            Err(PeerMcpError::Unbound)
        ));
    }
    assert!(calls.lock().expect("recorder").is_empty());
}

#[tokio::test]
async fn model_arguments_cannot_replace_attachment_sender_principal_or_session() {
    let backend = backend();
    let calls = backend.calls.clone();
    let adapter = PeerMcp::new(backend, config()).expect("valid config");
    bind(&adapter).await;
    for (tool, arguments) in model_calls() {
        for field in [
            "attachment",
            "sender_peer_id",
            "principal",
            "caller_id",
            "tenant_id",
            "session_id",
            "collaboration_ref",
        ] {
            let mut invalid = arguments.clone();
            invalid
                .as_object_mut()
                .expect("argument object")
                .insert(field.into(), json!("forged"));
            assert!(
                matches!(
                    adapter.invoke(request(tool, invalid)).await,
                    Err(PeerMcpError::InvalidArguments(_))
                ),
                "accepted {field} on {tool}"
            );
        }
    }
    assert_eq!(calls.lock().expect("recorder").len(), 2);
}

#[tokio::test]
async fn model_domain_values_are_validated_before_backend_access() {
    let backend = backend();
    let calls = backend.calls.clone();
    let adapter = PeerMcp::new(backend, config()).expect("valid config");
    bind(&adapter).await;
    for (tool, arguments) in [
        (
            "peer_send",
            json!({"recipient_peer_id":"display_alias","idempotency_key":"send_1","text":"body"}),
        ),
        (
            "peer_send",
            json!({"recipient_peer_id":remote_peer(),"idempotency_key":"send.bad","text":"body"}),
        ),
        (
            "peer_send",
            json!({"recipient_peer_id":remote_peer(),"idempotency_key":"send_1","text":""}),
        ),
        (
            "peer_send",
            json!({"recipient_peer_id":remote_peer(),"idempotency_key":"send_1","text":"é".repeat(1025)}),
        ),
        (
            "peer_send",
            json!({"recipient_peer_id":remote_peer(),"idempotency_key":"send_1","text":"body","reply_to_message_id":"message.bad"}),
        ),
        ("peer_status", json!({"idempotency_key":"send.bad"})),
        ("peer_acknowledge_batch", json!({"batch_id":"batch.bad"})),
    ] {
        assert!(matches!(
            adapter.invoke(request(tool, arguments)).await,
            Err(PeerMcpError::InvalidModelInput(_))
        ));
    }
    assert_eq!(calls.lock().expect("recorder").len(), 2);
}

#[tokio::test]
async fn acknowledgment_requires_the_backend_recipient_assignment() {
    let backend = backend();
    let acknowledged = backend.acknowledged.clone();
    let adapter = PeerMcp::new(backend, config()).expect("valid config");
    bind(&adapter).await;
    assert!(matches!(
        adapter
            .invoke(request("peer_acknowledge_batch", json!({"batch_id":"another_batch"})))
            .await,
        Err(PeerMcpError::Acknowledge(_))
    ));
    assert!(!acknowledged.load(Ordering::SeqCst));
}

#[tokio::test]
async fn backend_metadata_cannot_misattribute_sender_or_acknowledge_another_batch() {
    let mut wrong_sender = backend();
    wrong_sender
        .send_response
        .message
        .as_option_mut()
        .expect("message")
        .sender_peer_id = Some("c".repeat(64));
    let adapter = PeerMcp::new(wrong_sender, config()).expect("valid config");
    bind(&adapter).await;
    assert!(matches!(
        adapter.invoke(request("peer_send", model_calls().remove(1).1)).await,
        Err(PeerMcpError::InvalidMetadata)
    ));

    let mut wrong_batch = backend();
    wrong_batch
        .acknowledge_response
        .batch
        .as_option_mut()
        .expect("batch")
        .batch_id = Some("another_batch".into());
    let adapter = PeerMcp::new(wrong_batch, config()).expect("valid config");
    bind(&adapter).await;
    assert!(matches!(
        adapter
            .invoke(request("peer_acknowledge_batch", json!({"batch_id":"batch_1"})))
            .await,
        Err(PeerMcpError::InvalidMetadata)
    ));

    let mut missing_ack_fact = backend();
    missing_ack_fact
        .acknowledge_response
        .batch
        .as_option_mut()
        .expect("batch")
        .acknowledged_at = Default::default();
    let adapter = PeerMcp::new(missing_ack_fact, config()).expect("valid config");
    bind(&adapter).await;
    assert!(matches!(
        adapter
            .invoke(request("peer_acknowledge_batch", json!({"batch_id":"batch_1"})))
            .await,
        Err(PeerMcpError::InvalidMetadata)
    ));
}

#[test]
fn model_tool_schemas_have_no_identity_override_or_body_pull_surface() {
    let adapter = PeerMcp::new(backend(), config()).expect("valid config");
    for name in ["peer_roster", "peer_send", "peer_status", "peer_acknowledge_batch"] {
        let tool = ServerHandler::get_tool(&adapter, name).expect("advertised model operation");
        assert_eq!(tool.input_schema.get("additionalProperties"), Some(&json!(false)));
        for field in [
            "attachment",
            "sender_peer_id",
            "principal",
            "tenant_id",
            "session_id",
            "collaboration_ref",
        ] {
            assert!(tool.input_schema["properties"].get(field).is_none());
        }
    }
    for name in ["peer_inbox", "peer_receive", "ReceivePeerMessages"] {
        assert!(ServerHandler::get_tool(&adapter, name).is_none());
    }
}

#[tokio::test]
async fn sent_metadata_must_match_requested_recipient_size_and_expiry_window() {
    for (path, replacement) in [
        ("/message/recipientPeerId", json!("c".repeat(64))),
        ("/message/contentSizeBytes", json!(12)),
        ("/message/expiresAt", json!("2026-11-03T12:00:00Z")),
    ] {
        let mut backend = backend();
        let mut response = serde_json::to_value(&backend.send_response).expect("send response");
        *response.pointer_mut(path).expect("fixture field") = replacement;
        backend.send_response = serde_json::from_value(response).expect("wire response");
        let adapter = PeerMcp::new(backend, config()).expect("config");
        bind(&adapter).await;
        assert!(
            matches!(
                adapter.invoke(request("peer_send", model_calls().remove(1).1)).await,
                Err(PeerMcpError::InvalidMetadata)
            ),
            "accepted {path}"
        );
    }
}

#[tokio::test]
async fn roster_rejects_unknown_observations_and_nonpositive_lease_windows() {
    for (path, replacement) in [
        ("/peers/0/availability", json!("PEER_AVAILABILITY_UNSPECIFIED")),
        ("/peers/0/availability", json!(999)),
        ("/peers/0/deliveryCapability", json!("DELIVERY_CAPABILITY_UNSPECIFIED")),
        ("/peers/0/expiresAt", json!("2026-11-03T12:00:00Z")),
    ] {
        let mut backend = backend();
        let mut response = serde_json::to_value(&backend.roster_response).expect("roster response");
        *response.pointer_mut(path).expect("field") = replacement;
        backend.roster_response = serde_json::from_value(response).expect("wire response");
        let adapter = PeerMcp::new(backend, config()).expect("config");
        bind(&adapter).await;
        assert!(
            matches!(
                adapter.invoke(request("peer_roster", json!({}))).await,
                Err(PeerMcpError::InvalidMetadata)
            ),
            "accepted {path}"
        );
    }
    for condition in [json!("PEER_CONDITION_UNSPECIFIED"), json!(999)] {
        let mut backend = backend();
        let mut response = serde_json::to_value(&backend.roster_response).expect("roster response");
        response["peers"][0]["condition"] = condition;
        backend.roster_response = serde_json::from_value(response).expect("wire response");
        let adapter = PeerMcp::new(backend, config()).expect("config");
        bind(&adapter).await;
        assert!(matches!(
            adapter.invoke(request("peer_roster", json!({}))).await,
            Err(PeerMcpError::InvalidMetadata)
        ));
    }
}

#[tokio::test]
async fn roster_preserves_explicit_activity_without_inventing_progress() {
    let mut backend = backend();
    let mut response = serde_json::to_value(&backend.roster_response).expect("roster response");
    response["peers"][0]["activeAt"] = json!("2026-11-03T11:59:00Z");
    backend.roster_response = serde_json::from_value(response).expect("wire response");
    let adapter = PeerMcp::new(backend, config()).expect("config");
    bind(&adapter).await;
    let value = result_json(adapter.invoke(request("peer_roster", json!({}))).await.expect("roster"));
    assert_eq!(value["peers"][0]["activeAt"], "2026-11-03T11:59:00Z");
    assert_eq!(value["peers"][0]["observedAt"], "2026-11-03T12:00:00Z");
}

#[tokio::test]
async fn acknowledgment_rejects_duplicate_ids_and_time_before_preparation() {
    for (path, replacement) in [
        ("/batch/messageIds", json!(["incoming_1", "incoming_1"])),
        ("/batch/acknowledgedAt", json!("2026-11-03T11:59:59Z")),
    ] {
        let mut backend = backend();
        let mut response = serde_json::to_value(&backend.acknowledge_response).expect("ack response");
        *response.pointer_mut(path).expect("field") = replacement;
        backend.acknowledge_response = serde_json::from_value(response).expect("wire response");
        let adapter = PeerMcp::new(backend, config()).expect("config");
        bind(&adapter).await;
        assert!(
            matches!(
                adapter
                    .invoke(request("peer_acknowledge_batch", json!({"batch_id":"batch_1"})))
                    .await,
                Err(PeerMcpError::InvalidMetadata)
            ),
            "accepted {path}"
        );
    }
}

#[tokio::test]
async fn unspecified_or_unknown_message_status_is_not_presented_as_a_known_outcome() {
    for status in [json!("MESSAGE_STATUS_UNSPECIFIED"), json!(999)] {
        let mut backend = backend();
        let mut response = serde_json::to_value(&backend.status_response).expect("status response");
        response["status"] = status;
        backend.status_response = serde_json::from_value(response).expect("wire response");
        let adapter = PeerMcp::new(backend, config()).expect("config");
        bind(&adapter).await;
        assert!(matches!(
            adapter
                .invoke(request("peer_status", json!({"idempotency_key":"send_1"})))
                .await,
            Err(PeerMcpError::InvalidMetadata)
        ));
    }
}

#[tokio::test]
async fn malformed_service_metadata_has_a_safe_public_jsonrpc_diagnostic() {
    for invalid_encoding in [false, true] {
        let mut backend = backend();
        let metadata = backend.send_response.message.as_option_mut().expect("metadata");
        if invalid_encoding {
            metadata.accepted_at.as_option_mut().expect("accepted timestamp").nanos = -1;
        } else {
            metadata.sender_peer_id = Some("c".repeat(64));
        }
        let adapter = PeerMcp::new(backend, config()).expect("config");
        bind(&adapter).await;
        let (server_transport, client_transport) = tokio::io::duplex(32 * 1024);
        let server = tokio::spawn(async move {
            adapter
                .serve(server_transport)
                .await
                .expect("server handshake")
                .waiting()
                .await
                .expect("server shutdown")
        });
        let client = ().serve(client_transport).await.expect("client handshake");
        let result = client
            .call_tool(request("peer_send", model_calls().remove(1).1))
            .await
            .expect("tool result");
        client.cancel().await.expect("close client");
        server.await.expect("server task");
        let result = serde_json::to_value(result).expect("result JSON");
        assert_eq!(result["isError"], true);
        assert_eq!(
            result["content"][0]["text"],
            "Peer service returned invalid operation metadata."
        );
        assert!(!result.to_string().contains("internal_blob_ref"));
    }
}
