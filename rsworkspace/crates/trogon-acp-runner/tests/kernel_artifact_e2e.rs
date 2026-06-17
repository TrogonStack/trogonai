//! End-to-end test of the Fase 5 artifact wiring against a REAL NATS JetStream server.
//!
//! Requires Docker (uses testcontainers to spin up a NATS JetStream server). It drives a
//! synthetic turn — a tool call with a large output plus a Base64 image — through the
//! live `KernelConversationSink` with artifacts enabled, then asserts that the large
//! output and the image are persisted as artifact refs in the REAL object store
//! (retrievable + checksum-matched) and referenced from the canonical materialized state,
//! never inlined / flattened (§925-927, §966-1008; No-Lossy §2205).

use async_nats::jetstream;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_acp_runner::ReqwestImageFetcher;
use trogon_acp_runner::kernel_sink::{ConversationSink, KernelConversationSink};
use trogon_nats::jetstream::NatsJetStreamClient;
use trogon_tools::{ContentBlock, ImageSource, Message};
use trogonai_artifacts::{ArtifactStore, ArtifactStoreConfig, FetchLimits, provision_artifact_object_store};
use trogonai_session_contracts::SessionId;
use trogonai_session_contracts::__buffa::oneof::content_block::Kind as BlockKind;
use trogonai_session_contracts::__buffa::oneof::tool_call_result::Kind as ToolResultKind;
use trogonai_session_kernel::{
    EventLog, SessionKernel, SessionKernelConfig, SessionKernelOperationalPolicy, SessionKvLeaseFactory,
    SessionLeaseManager, SnapshotStore, provision_lease_store, provision_snapshot_store,
};

/// A 1x1 transparent PNG, base64-encoded (valid PNG magic bytes).
const ONE_PX_PNG_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    // NATS 2.11+ is required: the kernel snapshot/lease KV stores use per-key message TTL
    // (limit markers), which older NATS images do not support.
    let c = Nats::default()
        .with_tag("2.11-alpine")
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    (c, port)
}

#[tokio::test]
async fn large_tool_output_and_image_persist_as_artifact_refs_over_real_nats() {
    let (_container, port) = start_nats().await;
    let client = async_nats::connect(format!("127.0.0.1:{port}")).await.unwrap();
    let js = jetstream::new(client);

    // Provision the full kernel storage infra (snapshot KV, lease KV, event-log stream,
    // artifact object store) — the same provisioning the runner does at startup.
    let config = SessionKernelConfig::default();
    let snapshot_kv = provision_snapshot_store(&js, &config).await.unwrap();
    let lease_kv = provision_lease_store(&js, &config).await.unwrap();
    let js_client = NatsJetStreamClient::new(js.clone());
    let event_log = EventLog::new(js_client.clone(), js_client, config.clone());
    let policy = SessionKernelOperationalPolicy::default();
    event_log
        .provision_stream(&NatsJetStreamClient::new(js.clone()), &policy.nats)
        .await
        .unwrap();
    let snapshots = SnapshotStore::new(snapshot_kv, config.clone());
    let leases = SessionLeaseManager::new(SessionKvLeaseFactory::new(lease_kv, &config), "trogon-acp-test");
    let kernel = SessionKernel::new(config.clone(), event_log, snapshots, leases);

    let object_store = provision_artifact_object_store(&js, &config).await.unwrap();
    // A small inline limit so the moderate output genuinely claim-checks to the object
    // store (rather than being kept inline in the artifact metadata).
    let mut artifact_config = ArtifactStoreConfig::from_session_kernel(&config);
    artifact_config.inline_limit_bytes = 64;
    let artifact_store = ArtifactStore::new(object_store, artifact_config);
    let fetcher = ReqwestImageFetcher::new(&FetchLimits::default()).unwrap();

    // artifacts_enabled = true, inline_limit = 64 -> the producer claim-checks the output.
    let sink = KernelConversationSink::new(kernel.clone(), artifact_store.clone(), fetcher, true, 64);

    // A turn: a tool call whose output is large, plus a Base64 image in the same message.
    let big_output = "y".repeat(4096);
    let messages = vec![
        Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({ "cmd": "cat big.txt" }),
                    parent_tool_use_id: None,
                },
                ContentBlock::Image {
                    source: ImageSource::Base64 {
                        media_type: "image/png".to_string(),
                        data: ONE_PX_PNG_B64.to_string(),
                    },
                },
            ],
        },
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                content: big_output.clone(),
            }],
        },
    ];

    let sid = SessionId::new("sess_artifact_e2e").unwrap();
    sink.sync(sid.as_str(), &messages, "/repo").await;

    // Materialize the canonical state from the REAL event log + snapshot.
    let snapshot = kernel.materialize_state(&sid).await.unwrap();
    let state = snapshot.state.as_option().expect("materialized state present");

    // The large tool output is referenced as an artifact, not inlined as text.
    let call = state
        .tool_calls
        .iter()
        .find(|c| c.id == "t1")
        .expect("tool call materialized into canonical state");
    let result = call.result.as_option().expect("tool result present");
    let art_ref = match result.kind.as_ref() {
        Some(ToolResultKind::ArtifactRef(r)) => (**r).clone(),
        other => panic!("large tool output must be an artifact ref, got {other:?}"),
    };
    assert_eq!(art_ref.size_bytes as usize, big_output.len(), "ref must carry the full byte size");
    assert!(!art_ref.sha256.is_empty(), "ref must carry the checksum");

    // The bytes are actually retrievable from the REAL object store, and match the output.
    let retrieved = artifact_store
        .retrieve_by_ref(&sid, &art_ref)
        .await
        .expect("artifact must be retrievable from the real object store");
    assert_eq!(
        retrieved.content.as_ref(),
        big_output.as_bytes(),
        "retrieved artifact bytes must equal the original tool output"
    );

    // The image was persisted as an image_ref in the canonical transcript, not flattened.
    let has_image_ref = state
        .conversation
        .iter()
        .flat_map(|m| &m.content)
        .any(|b| matches!(b.kind.as_ref(), Some(BlockKind::ImageRef(_))));
    assert!(has_image_ref, "image must be persisted as an image_ref, not flattened to text");
}
