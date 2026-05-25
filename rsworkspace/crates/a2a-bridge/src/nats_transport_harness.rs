//! In-process harness for `A2A_BRIDGE_TRANSPORT=nats` wiring without a live NATS server.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use serde_json::json;

use a2a_nats::audit::emitter::{AuditEmitter, NatsAuditEmitter};
use a2a_nats::audit::envelope::{AuditEnvelope, AuditOutcome};
use a2a_nats::agent_id::A2aAgentId;
use a2a_auth_callout::CALLER_JWT_HEADER_NAME;
use a2a_nats::{A2aPrefix};

use crate::auth::{
    AuthCalloutJsonMintClient, BridgeTenantAccount, InProcessCalloutDispatcherMintWire,
    harness_callout_dispatcher,
};
use crate::error::BridgeError;
use crate::identity::BridgeUserJwt;
use crate::inbound::{
    AppState, GatewayInboundPublisher, GatewayUnaryPublish, ScriptedTaskJetstream, TaskJetStreamPort,
    default_a2a_prefix,
};

const HARNESS_CALLER_ID: &str = "bridge-harness-caller";
const HARNESS_TENANT: &str = "tenant-harness";

/// Gateway+agent stub for NATS transport integration tests.
#[derive(Clone)]
pub struct HarnessGatewayUnary {
    nats: trogon_nats::AdvancedMockNatsClient,
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
    last_caller_jwt_present: Arc<Mutex<bool>>,
    last_subject: Arc<Mutex<Option<String>>>,
}

impl HarnessGatewayUnary {
    #[must_use]
    pub fn new(
        nats: trogon_nats::AdvancedMockNatsClient,
        prefix: A2aPrefix,
        agent_id: A2aAgentId,
    ) -> Self {
        Self {
            nats,
            prefix,
            agent_id,
            last_caller_jwt_present: Arc::new(Mutex::new(false)),
            last_subject: Arc::new(Mutex::new(None)),
        }
    }

    #[must_use]
    pub fn last_caller_jwt_present(&self) -> bool {
        self.last_caller_jwt_present
            .lock()
            .ok()
            .is_some_and(|g| *g)
    }

    #[must_use]
    pub fn last_subject(&self) -> Option<String> {
        self.last_subject.lock().ok().and_then(|g| (*g).clone())
    }
}

#[async_trait]
impl GatewayUnaryPublish for HarnessGatewayUnary {
    async fn unary_request_gateway(
        &self,
        _caller_jwt: &BridgeUserJwt,
        subject: &str,
        headers: async_nats::HeaderMap,
        _payload: Bytes,
    ) -> Result<Bytes, BridgeError> {
        if let Ok(mut guard) = self.last_subject.lock() {
            *guard = Some(subject.to_owned());
        }
        let jwt_present = headers.get(CALLER_JWT_HEADER_NAME).is_some();
        if let Ok(mut guard) = self.last_caller_jwt_present.lock() {
            *guard = jwt_present;
        }

        let gateway_prefix = format!("{}.gateway.{}.", self.prefix.as_str(), self.agent_id.as_str());
        let method_dots = subject
            .strip_prefix(&gateway_prefix)
            .unwrap_or("message.send");
        let method_slashes = method_dots.replace('.', "/");
        let envelope = AuditEnvelope::new(
            &self.agent_id,
            method_slashes,
            None,
            0,
            0,
            AuditOutcome::Ok,
            None,
            Default::default(),
        );
        let emitter = NatsAuditEmitter::new(self.nats.clone());
        emitter.publish(&self.prefix, &self.agent_id, envelope).await;

        let body = if method_dots == "tasks.resubscribe" {
            json!({
                "jsonrpc": "2.0",
                "id": null,
                "result": { "taskId": "task-sse-1", "status": { "state": 1 } }
            })
        } else {
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": { "task": { "id": "task-unary-1", "status": { "state": 1 } } }
            })
        };
        Ok(Bytes::from(serde_json::to_vec(&body).unwrap()))
    }
}

/// Builds an [`AppState`] wired like `A2A_BRIDGE_TRANSPORT=nats` with in-process auth-callout mint.
#[must_use]
pub fn build_nats_transport_app_state(
    nats: trogon_nats::AdvancedMockNatsClient,
    agent_id: &str,
) -> (
    AppState,
    Arc<HarnessGatewayUnary>,
    Arc<InProcessCalloutDispatcherMintWire>,
) {
    let prefix = default_a2a_prefix();
    let agent = A2aAgentId::new(agent_id).expect("fixture agent id");
    let harness = Arc::new(HarnessGatewayUnary::new(nats, prefix.clone(), agent));
    let publisher = GatewayInboundPublisher::new(harness.clone());
    let jetstream: Arc<dyn TaskJetStreamPort> = Arc::new(ScriptedTaskJetstream::single_ok(
        json!({ "event": "task-status", "taskId": "task-sse-1" }).to_string(),
    ));

    let tenant = BridgeTenantAccount::new(HARNESS_TENANT).expect("harness tenant");
    let dispatcher = Arc::new(harness_callout_dispatcher(HARNESS_CALLER_ID));
    let mint_wire = Arc::new(InProcessCalloutDispatcherMintWire::new(dispatcher, tenant));
    let auth = Arc::new(AuthCalloutJsonMintClient::with_tenant_account(
        mint_wire.clone(),
        AuthCalloutJsonMintClient::<InProcessCalloutDispatcherMintWire>::default_mint_subject(),
        Some(BridgeTenantAccount::new(HARNESS_TENANT).expect("harness tenant")),
    ));

    let state = AppState::new(auth, Arc::new(publisher), jetstream, prefix);
    (state, harness, mint_wire)
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use axum::response::Response;
    use futures_util::StreamExt;
    use trogon_nats::AdvancedMockNatsClient;

    use a2a_nats::constants::GATEWAY_CALLER_ID_HTTP;

    use super::*;
    use crate::inbound::handle_jsonrpc;

    fn caller_headers(agent_id: &str, caller_id: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer fixture-token"),
        );
        headers.insert(
            axum::http::HeaderName::from_static("x-a2a-agent-id"),
            HeaderValue::from_str(agent_id).unwrap(),
        );
        if let Some(caller_id) = caller_id {
            headers.insert(
                axum::http::HeaderName::from_static(GATEWAY_CALLER_ID_HTTP),
                HeaderValue::from_str(caller_id).unwrap(),
            );
        }
        headers
    }

    async fn response_bytes(response: Response) -> Vec<u8> {
        to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()
    }

    #[tokio::test]
    async fn nats_transport_callout_mint_per_request_and_jwt_caller_id() {
        let nats = AdvancedMockNatsClient::new();
        let (state, harness, mint_wire) = build_nats_transport_app_state(nats.clone(), "planner");
        assert_eq!(mint_wire.mint_count(), 0);

        let body = Bytes::from(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "message/send",
                "params": { "message": { "messageId": "m1", "role": 1, "parts": [] } }
            })
            .to_string(),
        );

        let response = handle_jsonrpc(caller_headers("planner", None), body, &state)
            .await
            .expect("message/send should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(mint_wire.mint_count(), 1);
        assert!(harness.last_caller_jwt_present());
    }

    #[tokio::test]
    async fn nats_transport_message_send_round_trips_caller_jwt_and_audit() {
        let nats = AdvancedMockNatsClient::new();
        let (state, harness, mint_wire) = build_nats_transport_app_state(nats.clone(), "planner");
        let body = Bytes::from(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "message/send",
                "params": { "message": { "messageId": "m1", "role": 1, "parts": [] } }
            })
            .to_string(),
        );

        let response = handle_jsonrpc(caller_headers("planner", Some("caller-abc")), body, &state)
            .await
            .expect("message/send should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(mint_wire.mint_count(), 1);

        let payload = response_bytes(response).await;
        let parsed: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert!(parsed.get("result").is_some());

        assert!(harness.last_caller_jwt_present());
        assert_eq!(
            harness.last_subject().as_deref(),
            Some("a2a.gateway.planner.message.send")
        );

        let audit_subject = nats
            .published_messages()
            .into_iter()
            .find(|subject| subject.contains(".audit.ok.message.send"));
        assert!(audit_subject.is_some(), "expected gateway audit publish");
    }

    #[tokio::test]
    async fn nats_transport_tasks_resubscribe_bootstraps_sse_stream() {
        let nats = AdvancedMockNatsClient::new();
        let (state, harness, mint_wire) = build_nats_transport_app_state(nats.clone(), "planner");
        let body = Bytes::from(
            json!({
                "jsonrpc": "2.0",
                "id": "corr-1",
                "method": "tasks/resubscribe",
                "params": { "id": "task-sse-1", "last_seq": 0 }
            })
            .to_string(),
        );

        let response = handle_jsonrpc(caller_headers("planner", Some("caller-sse")), body, &state)
            .await
            .expect("tasks/resubscribe should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(mint_wire.mint_count(), 1);
        assert_eq!(
            response.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
            "text/event-stream"
        );

        assert!(harness.last_caller_jwt_present());
        assert_eq!(
            harness.last_subject().as_deref(),
            Some("a2a.gateway.planner.tasks.resubscribe")
        );

        let mut stream = response.into_body().into_data_stream();
        let mut saw_bootstrap = false;
        let mut saw_task_event = false;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.unwrap();
            let text = String::from_utf8_lossy(&chunk);
            if text.contains("gateway-bootstrap") {
                saw_bootstrap = true;
            }
            if text.contains("task-event") {
                saw_task_event = true;
            }
        }
        assert!(saw_bootstrap, "expected SSE gateway bootstrap line");
        assert!(saw_task_event, "expected SSE JetStream task event line");

        let audit_subject = nats
            .published_messages()
            .into_iter()
            .find(|subject| subject.contains(".audit.ok.tasks.resubscribe"));
        assert!(audit_subject.is_some(), "expected resubscribe audit publish");
    }

    /// Live NATS smoke — run with `A2A_SMOKE_COMPOSE=1 cargo test -p a2a-bridge -- --ignored nats_transport_live`.
    #[tokio::test]
    #[ignore = "requires compose stack: A2A_SMOKE_COMPOSE=1 and NATS_URL (see devops/docker/compose/compose.a2a.smoke.yml)"]
    async fn nats_transport_live_requires_nats_server() {
        if std::env::var("A2A_SMOKE_COMPOSE").as_deref() != Ok("1") {
            panic!("set A2A_SMOKE_COMPOSE=1 to run live compose-network bridge smoke");
        }
        let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
        let client = async_nats::connect(url).await.expect("live NATS connect");
        let _ = client;
        let bridge_addr = std::env::var("BRIDGE_LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:7443".into());
        let response = reqwest::get(format!("http://{bridge_addr}/"))
            .await
            .expect("bridge HTTP reachable");
        assert!(
            response.status().is_client_error() || response.status().is_success(),
            "bridge HTTP should respond on {bridge_addr}"
        );
    }
}
