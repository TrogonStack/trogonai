use bytes::Bytes;
use futures_util::future::BoxFuture;
use std::sync::Arc;
use trogon_transcript::TranscriptEntry;
use trogon_transcript::entry::{Role, now_ms};

use crate::error::ActorError;

// Type aliases for the two injected operations.
type AppendFn =
    Arc<dyn Fn(TranscriptEntry) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
type SpawnFn =
    Arc<dyn Fn(String, Bytes) -> BoxFuture<'static, Result<Bytes, String>> + Send + Sync>;

/// Passed to every [`crate::EntityActor::handle`] invocation.
///
/// Gives the actor:
/// - Read access to its entity key and actor type.
/// - Convenience methods for recording transcript entries.
/// - [`spawn_agent`][ActorContext::spawn_agent] for delegating work to a
///   sub-agent via NATS request-reply.
///
/// `ActorContext` is non-generic (all infrastructure is type-erased behind
/// function pointers) so actor implementations don't need to carry extra
/// generic parameters.
pub struct ActorContext {
    /// The entity key this invocation is handling, e.g. `"owner/repo/456"`.
    pub entity_key: String,
    /// The actor type string, e.g. `"pr"`.
    pub actor_type: String,
    /// The unique ID of the transcript session opened for this invocation.
    pub session_id: String,
    append_fn: AppendFn,
    spawn_fn: SpawnFn,
}

impl ActorContext {
    /// Construct a context by injecting the append and spawn functions.
    ///
    /// Called by the runtime — actor implementors don't construct this directly.
    pub fn new(
        entity_key: impl Into<String>,
        actor_type: impl Into<String>,
        session_id: impl Into<String>,
        append_fn: AppendFn,
        spawn_fn: SpawnFn,
    ) -> Self {
        Self {
            entity_key: entity_key.into(),
            actor_type: actor_type.into(),
            session_id: session_id.into(),
            append_fn,
            spawn_fn,
        }
    }

    // ── Transcript helpers ────────────────────────────────────────────────────
    //
    // These return `Result<(), String>` — a transcript write failure is never
    // a reason to abort an actor invocation. Actors typically call `.ok()` on
    // the result and continue.

    /// Append an arbitrary transcript entry.
    pub async fn append(&self, entry: TranscriptEntry) -> Result<(), String> {
        (self.append_fn)(entry).await
    }

    /// Record a user-role message in the transcript.
    pub async fn append_user_message(
        &self,
        content: impl Into<String>,
        tokens: Option<u32>,
    ) -> Result<(), String> {
        self.append(TranscriptEntry::Message {
            role: Role::User,
            content: content.into(),
            timestamp: now_ms(),
            tokens,
        })
        .await
    }

    /// Record an assistant-role message in the transcript.
    pub async fn append_assistant_message(
        &self,
        content: impl Into<String>,
        tokens: Option<u32>,
    ) -> Result<(), String> {
        self.append(TranscriptEntry::Message {
            role: Role::Assistant,
            content: content.into(),
            timestamp: now_ms(),
            tokens,
        })
        .await
    }

    /// Record a tool call in the transcript.
    pub async fn append_tool_call(
        &self,
        name: impl Into<String>,
        input: serde_json::Value,
        output: serde_json::Value,
        duration_ms: u64,
    ) -> Result<(), String> {
        self.append(TranscriptEntry::ToolCall {
            name: name.into(),
            input,
            output,
            duration_ms,
            timestamp: now_ms(),
        })
        .await
    }

    // ── Sub-agent spawning ────────────────────────────────────────────────────

    /// Delegate a unit of work to a sub-agent and await its reply.
    ///
    /// The runtime looks up an agent with the given `capability` in the live
    /// registry, sends `payload` via NATS request-reply to that agent's inbox,
    /// records a [`TranscriptEntry::SubAgentSpawn`] entry, and returns the
    /// reply payload.
    ///
    /// Returns `Err` if no registered agent provides the capability or if the
    /// request-reply fails.
    pub async fn spawn_agent<E: std::error::Error>(
        &self,
        capability: impl Into<String>,
        payload: Bytes,
    ) -> Result<Bytes, ActorError<E>> {
        let cap = capability.into();
        (self.spawn_fn)(cap, payload)
            .await
            .map_err(ActorError::SpawnFailed)
    }
}

// ── Test helpers ──────────────────────────────────────────────────────────────

#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers {
    use super::*;
    use std::sync::Mutex;

    /// Build a testable `ActorContext` that collects appended entries in memory
    /// and returns a fixed `Bytes` response for every `spawn_agent` call.
    pub struct ContextBuilder {
        entity_key: String,
        actor_type: String,
        spawn_response: Bytes,
    }

    impl ContextBuilder {
        pub fn new(actor_type: impl Into<String>, entity_key: impl Into<String>) -> Self {
            Self {
                actor_type: actor_type.into(),
                entity_key: entity_key.into(),
                spawn_response: Bytes::new(),
            }
        }

        pub fn with_spawn_response(mut self, response: impl Into<Bytes>) -> Self {
            self.spawn_response = response.into();
            self
        }

        /// Build the context. Returns the context and a shared handle to the
        /// collected transcript entries.
        pub fn build(self) -> (ActorContext, Arc<Mutex<Vec<TranscriptEntry>>>) {
            let entries: Arc<Mutex<Vec<TranscriptEntry>>> =
                Arc::new(Mutex::new(Vec::new()));
            let entries_clone = Arc::clone(&entries);

            let append_fn: AppendFn = Arc::new(move |entry| {
                let e = Arc::clone(&entries_clone);
                Box::pin(async move {
                    e.lock().unwrap().push(entry);
                    Ok(())
                })
            });

            let spawn_response = self.spawn_response.clone();
            let spawn_fn: SpawnFn = Arc::new(move |_cap, _payload| {
                let resp = spawn_response.clone();
                Box::pin(async move { Ok(resp) })
            });

            let ctx = ActorContext::new(
                self.entity_key,
                self.actor_type,
                uuid::Uuid::new_v4().to_string(),
                append_fn,
                spawn_fn,
            );

            (ctx, entries)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::ContextBuilder;
    use trogon_transcript::TranscriptEntry;

    #[tokio::test]
    async fn append_user_message_records_entry() {
        let (ctx, entries) = ContextBuilder::new("pr", "owner/repo/1").build();
        ctx.append_user_message("hello", None).await.unwrap();
        let snapshot = entries.lock().unwrap();
        assert_eq!(snapshot.len(), 1);
        assert!(matches!(snapshot[0], TranscriptEntry::Message { .. }));
    }

    #[tokio::test]
    async fn spawn_agent_returns_configured_response() {
        let (ctx, _) = ContextBuilder::new("pr", "owner/repo/1")
            .with_spawn_response(b"ok".as_ref())
            .build();
        let result = ctx
            .spawn_agent::<std::convert::Infallible>("security_analysis", bytes::Bytes::new())
            .await
            .unwrap();
        assert_eq!(result.as_ref(), b"ok");
    }

    #[tokio::test]
    async fn append_assistant_message_records_entry() {
        let (ctx, entries) = ContextBuilder::new("pr", "owner/repo/1").build();
        ctx.append_assistant_message("LGTM", Some(12)).await.unwrap();
        let snapshot = entries.lock().unwrap();
        assert_eq!(snapshot.len(), 1);
        assert!(matches!(
            &snapshot[0],
            TranscriptEntry::Message { role: trogon_transcript::entry::Role::Assistant, tokens: Some(12), .. }
        ));
    }

    #[tokio::test]
    async fn append_tool_call_records_entry() {
        let (ctx, entries) = ContextBuilder::new("pr", "owner/repo/1").build();
        ctx.append_tool_call(
            "search",
            serde_json::json!({"q": "foo"}),
            serde_json::json!({"results": []}),
            55,
        )
        .await
        .unwrap();
        let snapshot = entries.lock().unwrap();
        assert_eq!(snapshot.len(), 1);
        assert!(matches!(
            &snapshot[0],
            TranscriptEntry::ToolCall { name, duration_ms: 55, .. } if name == "search"
        ));
    }

}
