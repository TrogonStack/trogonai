use bytes::Bytes;
use futures_util::future::BoxFuture;
use std::sync::Arc;
use trogon_transcript::TranscriptEntry;
use trogon_transcript::entry::{Role, now_ms};

use crate::error::ActorError;

/// Maximum number of nested `spawn_agent` hops allowed in a single call chain.
///
/// When actor A spawns B, B is given `spawn_depth = 1`. If B also calls
/// `spawn_agent`, C would receive `spawn_depth = 2`. Once the depth reaches
/// this limit the call is rejected with `ActorError::SpawnFailed` to prevent
/// unbounded recursion.
pub const MAX_SPAWN_DEPTH: u32 = 5;

// Type aliases for the three injected operations.
type AppendFn =
    Arc<dyn Fn(TranscriptEntry) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
// SpawnFn carries the *next* depth (current + 1) so it can embed it in the
// NATS request headers for the child actor to read.
type SpawnFn =
    Arc<dyn Fn(String, Bytes, u32) -> BoxFuture<'static, Result<Bytes, String>> + Send + Sync>;
// RecallFn fetches and formats the entity's past transcript for the current actor.
pub(crate) type RecallFn = Arc<dyn Fn() -> BoxFuture<'static, Option<String>> + Send + Sync>;

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
    /// How many `spawn_agent` hops deep this invocation is.
    ///
    /// Top-level actors (dispatched by the router) start at `0`. Each nested
    /// spawn increments by one. Calls that would exceed [`MAX_SPAWN_DEPTH`] are
    /// rejected immediately to prevent unbounded recursion.
    pub spawn_depth: u32,
    append_fn: AppendFn,
    spawn_fn: SpawnFn,
    recall_fn: Option<RecallFn>,
}

impl ActorContext {
    /// Construct a context by injecting the append, spawn, and recall functions.
    ///
    /// Called by the runtime — actor implementors don't construct this directly.
    pub fn new(
        entity_key: impl Into<String>,
        actor_type: impl Into<String>,
        session_id: impl Into<String>,
        spawn_depth: u32,
        append_fn: AppendFn,
        spawn_fn: SpawnFn,
        recall_fn: Option<RecallFn>,
    ) -> Self {
        Self {
            entity_key: entity_key.into(),
            actor_type: actor_type.into(),
            session_id: session_id.into(),
            spawn_depth,
            append_fn,
            spawn_fn,
            recall_fn,
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
    /// Returns `Err` if:
    /// - No registered agent provides the capability.
    /// - The request-reply fails or times out.
    /// - The call would exceed [`MAX_SPAWN_DEPTH`] (prevents infinite recursion).
    pub async fn spawn_agent<E: std::error::Error>(
        &self,
        capability: impl Into<String>,
        payload: Bytes,
    ) -> Result<Bytes, ActorError<E>> {
        if self.spawn_depth >= MAX_SPAWN_DEPTH {
            return Err(ActorError::SpawnFailed(format!(
                "spawn depth limit ({MAX_SPAWN_DEPTH}) exceeded — aborting recursive spawn"
            )));
        }
        let cap = capability.into();
        let next_depth = self.spawn_depth + 1;
        (self.spawn_fn)(cap, payload, next_depth)
            .await
            .map_err(ActorError::SpawnFailed)
    }

    // ── Entity history recall ─────────────────────────────────────────────────

    /// Return a formatted block of past transcript messages for this entity, or
    /// `None` if no transcript reader is configured or no prior messages exist.
    ///
    /// Call this early in `handle()` — before writing new transcript entries —
    /// and prepend the result to your LLM prompt so the model has context from
    /// previous sessions.
    pub async fn recall_entity_history(&self) -> Option<String> {
        match &self.recall_fn {
            Some(f) => f().await,
            None => None,
        }
    }
}

// ── Test helpers ──────────────────────────────────────────────────────────────

#[cfg(any(test, feature = "test-support"))]
pub mod test_helpers {
    use super::*;
    use std::sync::Mutex;

    /// Build a testable `ActorContext` that collects appended entries in memory
    /// and returns a fixed `Bytes` response for every `spawn_agent` call.
    pub struct ContextBuilder {
        entity_key: String,
        actor_type: String,
        spawn_response: Bytes,
        recall_result: Option<String>,
    }

    impl ContextBuilder {
        pub fn new(actor_type: impl Into<String>, entity_key: impl Into<String>) -> Self {
            Self {
                actor_type: actor_type.into(),
                entity_key: entity_key.into(),
                spawn_response: Bytes::new(),
                recall_result: None,
            }
        }

        pub fn with_spawn_response(mut self, response: impl Into<Bytes>) -> Self {
            self.spawn_response = response.into();
            self
        }

        /// Configure the fixed string that `recall_entity_history()` will return.
        pub fn with_recall_result(mut self, result: impl Into<String>) -> Self {
            self.recall_result = Some(result.into());
            self
        }

        /// Build the context. Returns the context and a shared handle to the
        /// collected transcript entries.
        pub fn build(self) -> (ActorContext, Arc<Mutex<Vec<TranscriptEntry>>>) {
            let entries: Arc<Mutex<Vec<TranscriptEntry>>> = Arc::new(Mutex::new(Vec::new()));
            let entries_clone = Arc::clone(&entries);

            let append_fn: AppendFn = Arc::new(move |entry| {
                let e = Arc::clone(&entries_clone);
                Box::pin(async move {
                    e.lock().unwrap().push(entry);
                    Ok(())
                })
            });

            let spawn_response = self.spawn_response.clone();
            let spawn_fn: SpawnFn = Arc::new(move |_cap, _payload, _depth| {
                let resp = spawn_response.clone();
                Box::pin(async move { Ok(resp) })
            });

            let recall_fn: Option<RecallFn> = self.recall_result.map(|s| {
                let result: RecallFn = Arc::new(move || {
                    let s = s.clone();
                    Box::pin(async move { Some(s) })
                });
                result
            });

            let ctx = ActorContext::new(
                self.entity_key,
                self.actor_type,
                uuid::Uuid::new_v4().to_string(),
                0, // spawn_depth: top-level test context
                append_fn,
                spawn_fn,
                recall_fn,
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
    async fn recall_entity_history_returns_some_when_configured() {
        let (ctx, _) = ContextBuilder::new("pr", "owner/repo/1")
            .with_recall_result("## Entity history (1 messages)\n\nuser: hello")
            .build();
        let result = ctx.recall_entity_history().await;
        assert!(result.is_some());
        assert!(result.unwrap().contains("user: hello"));
    }

    #[tokio::test]
    async fn recall_entity_history_returns_none_without_recall_fn() {
        let (ctx, _) = ContextBuilder::new("pr", "owner/repo/1").build();
        assert!(ctx.recall_entity_history().await.is_none());
    }

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
        ctx.append_assistant_message("LGTM", Some(12))
            .await
            .unwrap();
        let snapshot = entries.lock().unwrap();
        assert_eq!(snapshot.len(), 1);
        assert!(matches!(
            &snapshot[0],
            TranscriptEntry::Message {
                role: trogon_transcript::entry::Role::Assistant,
                tokens: Some(12),
                ..
            }
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
