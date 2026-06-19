//! Integration test for the IDE multi-runner permission relay slice.
//!
//! Proves the round-trip: a permission request published to an *external runner's* prefix
//! (`xai.session.{runner_sid}.client.session.request_permission`) is dispatched by
//! `client::run`, remapped `runner_sid -> acp_sid` by `RemappingClient`, delivered to the
//! IDE client with the acp session id, and the decision is returned over the NATS reply.
//!
//! This is the vertical slice that validates the permission design with real code:
//! - `RemappingClient` satisfies `client::run`'s `Cl: Client + ElicitationClient` bounds.
//! - A `client::run` running for a non-default runner prefix works.
//! - The session-id remap is applied to the IDE-bound request, and the reply path is
//!   unaffected by it.
//!
//! Requires Docker (testcontainers NATS).
//!   cargo test -p acp-nats --test remapping_client_integration

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acp_nats::{AcpPrefix, Bridge, Config, IdRemap, NatsAuth, NatsConfig, RemappingClient, StdJsonSerialize, client};
use agent_client_protocol::{
    Client, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse, SessionId,
    SessionNotification, ToolCallUpdate, ToolCallUpdateFields,
};
use async_trait::async_trait;
use bytes::Bytes;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, runners::AsyncRunner};
use trogon_std::time::SystemClock;

/// IDE-side client stand-in that records the `session_id` it actually received.
struct CapturingClient {
    seen_session_id: Arc<Mutex<Option<String>>>,
}

#[async_trait(?Send)]
impl Client for CapturingClient {
    async fn request_permission(
        &self,
        args: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        *self.seen_session_id.lock().unwrap() = Some(args.session_id.0.to_string());
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    async fn session_notification(&self, _: SessionNotification) -> agent_client_protocol::Result<()> {
        Ok(())
    }
}

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

async fn nats_client(port: u16) -> async_nats::Client {
    async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("Failed to connect to NATS")
}

fn make_bridge(
    nats: async_nats::Client,
    prefix: &str,
) -> Bridge<async_nats::Client, SystemClock, trogon_nats::jetstream::NatsJetStreamClient> {
    let config = Config::new(
        AcpPrefix::new(prefix).unwrap(),
        NatsConfig {
            servers: vec!["unused".to_string()],
            auth: NatsAuth::None,
        },
    )
    .with_operation_timeout(Duration::from_millis(500));
    let js_client = trogon_nats::jetstream::NatsJetStreamClient::new(async_nats::jetstream::new(nats.clone()));
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    Bridge::new(
        nats,
        js_client,
        SystemClock,
        &opentelemetry::global::meter("remapping-client-test"),
        config,
        tx,
    )
}

#[tokio::test]
async fn permission_request_is_relayed_with_remapped_session_id() {
    let (_container, port) = start_nats().await;
    let nats_runner = nats_client(port).await; // simulates the external runner's NatsClientProxy
    let nats_relay = nats_client(port).await; // the client::run relay

    // The external runner registers under its own prefix and its own session id; the IDE
    // tracks the same conversation under the acp session id.
    let runner_prefix = "xai";
    let runner_sid = "runner-sid-1";
    let acp_sid = "acp-sid-1";

    let bridge = make_bridge(nats_relay.clone(), runner_prefix);
    let seen = Arc::new(Mutex::new(None));
    let seen_clone = seen.clone();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let inner = Rc::new(CapturingClient {
                seen_session_id: seen_clone,
            });
            let id_remap: IdRemap = Rc::new(RefCell::new(HashMap::new()));
            id_remap
                .borrow_mut()
                .insert(runner_sid.to_string(), acp_sid.to_string());
            let relay_client = Rc::new(RemappingClient::new(inner, id_remap));
            let bridge_rc = Rc::new(bridge);

            tokio::task::spawn_local(async move {
                client::run(nats_relay, relay_client, bridge_rc, StdJsonSerialize).await;
            });

            // Let the proxy subscribe before publishing.
            tokio::time::sleep(Duration::from_millis(150)).await;

            // The runner emits request_permission carrying its own runner_sid (raw-JSON
            // protocol, matching NatsClientProxy). The body session id must match the
            // subject's session segment — both runner_sid.
            let req = RequestPermissionRequest::new(
                SessionId::from(runner_sid.to_string()),
                ToolCallUpdate::new("call-1", ToolCallUpdateFields::new()),
                vec![],
            );
            let payload = serde_json::to_vec(&req).unwrap();
            let subject = format!("{runner_prefix}.session.{runner_sid}.client.session.request_permission");
            let reply = nats_runner
                .request(subject, Bytes::from(payload))
                .await
                .expect("permission request must get a reply");

            let response: RequestPermissionResponse =
                serde_json::from_slice(&reply.payload).expect("response must deserialize");
            assert_eq!(
                response.outcome,
                RequestPermissionOutcome::Cancelled,
                "the IDE client's decision must flow back to the runner over the NATS reply"
            );
        })
        .await;

    assert_eq!(
        seen.lock().unwrap().as_deref(),
        Some(acp_sid),
        "the IDE client must receive the remapped acp session id, not the runner's session id"
    );
}
