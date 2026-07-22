//! In-process harness for `A2A_BRIDGE_TRANSPORT=nats` wiring without a live NATS server.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use serde_json::json;

use a2a_auth_callout::CALLER_JWT_HEADER_NAME;
use a2a_nats::A2aPrefix;
use a2a_nats::agent_id::A2aAgentId;
use a2a_nats::audit::emitter::{AuditEmitter, NatsAuditEmitter};
use a2a_nats::audit::envelope::{AuditEnvelope, AuditOutcome};

use crate::auth::{
    AuthCalloutJsonMintClient, BridgeTenantAccount, InProcessCalloutDispatcherMintWire, harness_callout_dispatcher,
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
    pub fn new(nats: trogon_nats::AdvancedMockNatsClient, prefix: A2aPrefix, agent_id: A2aAgentId) -> Self {
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
        // Poisoned mutex means a panic landed elsewhere — propagate
        // by panicking here so the test surfaces the root cause
        // instead of silently reading "no jwt".
        *self.last_caller_jwt_present.lock().expect("harness mutex poisoned")
    }

    #[must_use]
    pub fn last_subject(&self) -> Option<String> {
        self.last_subject.lock().expect("harness mutex poisoned").clone()
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
        *self.last_subject.lock().expect("harness mutex poisoned") = Some(subject.to_owned());
        let jwt_present = headers.get(CALLER_JWT_HEADER_NAME).is_some();
        *self.last_caller_jwt_present.lock().expect("harness mutex poisoned") = jwt_present;

        let gateway_prefix = format!("{}.gateway.{}.", self.prefix.as_str(), self.agent_id.as_str());
        let method_dots = subject.strip_prefix(&gateway_prefix).unwrap_or("message.send");
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
mod tests;
