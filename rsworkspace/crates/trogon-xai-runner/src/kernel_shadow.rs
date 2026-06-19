//! Fase 4 (§1875 "runners como bindings/adapters") shadow emission for the xAI runner.
//!
//! The session belongs to Trogonai, not the runner (§ Principio central). Each completed
//! turn's conversation is mirrored, in parallel with the runner's own KV snapshot, into
//! the canonical Session Kernel event log + snapshot (§1671 shadow mode: "emitir eventos
//! en paralelo"; §3 event log; §11 / § No-Lossy Contract: full role + content preserved as
//! canonical truth). The runner converts ITS OWN snapshot messages into canonical messages
//! — it does not share a sink with other runners.
//!
//! Gated by the kernel feature flags (off by default) and strictly best-effort: a kernel
//! failure is logged and never blocks or breaks the prompt path.

use std::sync::Arc;

use async_trait::async_trait;
use buffa::EnumValue;
use buffa_types::google::protobuf::Timestamp;
use trogon_nats::jetstream::NatsJetStreamClient;
use trogonai_session_contracts::{
    Actor, ActorType, CanonicalMessage, ContentBlock, SessionId,
    __buffa::oneof::content_block::Kind as BlockKind,
};
use trogonai_session_kernel::{
    EventLog, EventLogBackend, SessionKernel, SessionKernelConfig, SessionKernelFeatureFlags,
    SessionKernelOperationalPolicy, SessionKvLeaseFactory, SessionLeaseFactory, SessionLeaseManager, SnapshotStore,
    provision_lease_store, provision_snapshot_store,
};

use crate::session_store::SessionSnapshot;

/// Records a completed turn's conversation into the canonical kernel (shadow mode).
#[async_trait]
pub trait ShadowRecorder: Send + Sync {
    async fn record_turn(&self, session_id: &str, snapshot: &SessionSnapshot);
}

/// Convert the xAI runner's own snapshot messages into canonical messages (§11 no-lossy:
/// full role and text content preserved; xAI snapshot messages are text-only). The
/// `message_id` is positional + stable so event-log replay is deterministic.
pub fn canonical_messages(snapshot: &SessionSnapshot) -> Vec<CanonicalMessage> {
    snapshot
        .messages
        .iter()
        .enumerate()
        .map(|(idx, message)| CanonicalMessage {
            message_id: format!("m{idx}"),
            role: message.role.clone(),
            content: message
                .content
                .iter()
                .map(|block| ContentBlock {
                    kind: Some(BlockKind::Text(block.text.clone())),
                    ..ContentBlock::default()
                })
                .collect(),
            model: snapshot.model.clone(),
            ..CanonicalMessage::default()
        })
        .collect()
}

fn shadow_actor() -> Actor {
    Actor {
        r#type: EnumValue::Known(ActorType::Runner),
        id: "trogon-xai-runner".to_string(),
        ..Actor::default()
    }
}

fn now_timestamp() -> Timestamp {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Timestamp {
        seconds: elapsed.as_secs() as i64,
        nanos: elapsed.subsec_nanos() as i32,
        ..Default::default()
    }
}

/// Kernel-backed recorder: mirrors a turn's messages into the canonical event log +
/// snapshot. Generic over the kernel backends; constructed once in [`provision`] and used
/// behind `Arc<dyn ShadowRecorder>` so the concrete types never leak into the agent.
struct KernelShadowRecorder<E, S, L> {
    kernel: SessionKernel<E, S, L>,
}

#[async_trait]
impl<E, S, L> ShadowRecorder for KernelShadowRecorder<E, S, L>
where
    E: EventLogBackend,
    S: trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + Clone
        + Send
        + Sync
        + 'static,
    L: SessionLeaseFactory + Clone + Send + Sync + 'static,
{
    async fn record_turn(&self, session_id: &str, snapshot: &SessionSnapshot) {
        let Ok(sid) = SessionId::new(session_id) else {
            return;
        };
        let messages = canonical_messages(snapshot);
        if messages.is_empty() {
            return;
        }
        if let Err(err) = self
            .kernel
            .record_conversation(&sid, &messages, shadow_actor(), now_timestamp())
            .await
        {
            tracing::warn!(session_id, error = %err, "xai kernel shadow: record_conversation failed (non-fatal)");
        }
    }
}

/// Provision the canonical shadow recorder when the kernel is enabled. Returns `None`
/// (legacy path, no canonical recording) by default or if provisioning fails — the runner
/// never blocks on the kernel. Mirrors the acp-runner sink's gating: the shadow event log
/// (Fase 4) is recorded under a session lease (Fase 2) and materialized into the snapshot
/// (Fase 3), so it is gated by both `lease_enabled()` and `snapshot_enabled()` (each folds
/// in the master `session_kernel_enabled` flag).
pub async fn provision(js: &async_nats::jetstream::Context) -> Option<Arc<dyn ShadowRecorder>> {
    let flags = SessionKernelFeatureFlags::default();
    if !flags.lease_enabled() || !flags.snapshot_enabled() {
        return None;
    }

    let config = SessionKernelConfig::default();
    let snapshot_kv = match provision_snapshot_store(js, &config).await {
        Ok(kv) => kv,
        Err(err) => {
            tracing::warn!(error = %err, "xai kernel shadow: snapshot store provisioning failed; disabled");
            return None;
        }
    };
    let lease_kv = match provision_lease_store(js, &config).await {
        Ok(kv) => kv,
        Err(err) => {
            tracing::warn!(error = %err, "xai kernel shadow: lease store provisioning failed; disabled");
            return None;
        }
    };

    let js_client = NatsJetStreamClient::new(js.clone());
    let event_log = EventLog::new(js_client.clone(), js_client, config.clone());
    let operational_policy = SessionKernelOperationalPolicy::default();
    if let Err(err) = event_log
        .provision_stream(&NatsJetStreamClient::new(js.clone()), &operational_policy.nats)
        .await
    {
        tracing::warn!(error = %err, "xai kernel shadow: event-log stream provisioning failed; disabled");
        return None;
    }
    let snapshots = SnapshotStore::new(snapshot_kv, config.clone());
    let leases = SessionLeaseManager::new(SessionKvLeaseFactory::new(lease_kv, &config), "trogon-xai-runner");
    let kernel = SessionKernel::new(config, event_log, snapshots, leases);

    tracing::info!("xai kernel shadow: canonical conversation shadow recording enabled");
    Some(Arc::new(KernelShadowRecorder { kernel }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_store::{SnapshotMessage, TextBlock};

    fn snapshot_with(messages: Vec<SnapshotMessage>, model: Option<&str>) -> SessionSnapshot {
        SessionSnapshot {
            id: "s1".to_string(),
            tenant_id: "t1".to_string(),
            name: "n".to_string(),
            model: model.map(str::to_string),
            compactor_provider: None,
            compactor_model: None,
            needs_compactor_migration: false,
            tools: vec![],
            memory_path: None,
            messages,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            agent_id: None,
            parent_session_id: None,
            branched_at_index: None,
            mcp_servers: vec![],
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_read_tokens: 0,
        }
    }

    #[test]
    fn canonical_messages_preserve_role_text_and_model() {
        let snapshot = snapshot_with(
            vec![
                SnapshotMessage {
                    role: "user".to_string(),
                    content: vec![TextBlock::new("hello")],
                    usage: None,
                },
                SnapshotMessage {
                    role: "assistant".to_string(),
                    content: vec![TextBlock::new("hi there")],
                    usage: None,
                },
            ],
            Some("grok-3"),
        );

        let canonical = canonical_messages(&snapshot);
        assert_eq!(canonical.len(), 2);
        assert_eq!(canonical[0].role, "user");
        assert_eq!(canonical[0].message_id, "m0");
        assert_eq!(canonical[0].model.as_deref(), Some("grok-3"));
        match canonical[1].content[0].kind.as_ref().unwrap() {
            BlockKind::Text(text) => assert_eq!(text, "hi there"),
            other => panic!("expected text block, got {other:?}"),
        }
    }

    #[test]
    fn canonical_messages_empty_when_no_messages() {
        assert!(canonical_messages(&snapshot_with(vec![], Some("grok-3"))).is_empty());
    }
}
