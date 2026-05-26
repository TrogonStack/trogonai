use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use trogon_tools::ToolDef;
use trogon_mcp::McpCallTool;

use crate::session_store::{SessionStore, TodoItem};

/// Implements `todo_write` and `todo_read` backed by the session's NATS KV entry.
///
/// Todos are stored inside [`SessionState::todos`] so they share the same
/// persistence as messages and terminal state — no separate KV bucket needed.
pub struct NatsTodoTool<S> {
    session_id: String,
    store: S,
}

impl<S: SessionStore> NatsTodoTool<S> {
    pub fn new(session_id: impl Into<String>, store: S) -> Self {
        Self {
            session_id: session_id.into(),
            store,
        }
    }

    pub fn todo_write_def() -> ToolDef {
        ToolDef {
            name: "todo_write".to_string(),
            description:
                "Create or update a todo item. Status must be 'pending', 'in_progress', or 'completed'."
                    .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "id":      { "type": "string", "description": "Unique identifier for the todo" },
                    "content": { "type": "string", "description": "Description of the task" },
                    "status":  { "type": "string", "description": "One of: pending, in_progress, completed" }
                },
                "required": ["id", "content", "status"]
            }),
            cache_control: None,
        }
    }

    pub fn todo_read_def() -> ToolDef {
        ToolDef {
            name: "todo_read".to_string(),
            description: "List all active (non-completed) todos for the current session."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            cache_control: None,
        }
    }

    pub fn into_dispatches(self) -> Vec<(String, String, Arc<dyn McpCallTool>)>
    where
        S: Send + Sync + 'static,
    {
        let shared: Arc<dyn McpCallTool> = Arc::new(self);
        vec![
            ("todo_write".to_string(), "todo_write".to_string(), Arc::clone(&shared)),
            ("todo_read".to_string(), "todo_read".to_string(), shared),
        ]
    }
}

impl<S: SessionStore> McpCallTool for NatsTodoTool<S> {
    fn call_tool<'a>(
        &'a self,
        name: &'a str,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        let session_id = self.session_id.clone();
        let store = self.store.clone();
        let name = name.to_string();
        let arguments = arguments.clone();

        Box::pin(async move {
            match name.as_str() {
                "todo_write" => {
                    let id = arguments["id"]
                        .as_str()
                        .ok_or_else(|| "missing 'id'".to_string())?
                        .to_string();
                    let content = arguments["content"]
                        .as_str()
                        .ok_or_else(|| "missing 'content'".to_string())?
                        .to_string();
                    let status = arguments["status"]
                        .as_str()
                        .ok_or_else(|| "missing 'status'".to_string())?
                        .to_string();

                    if !matches!(status.as_str(), "pending" | "in_progress" | "completed") {
                        return Err(format!(
                            "status must be 'pending', 'in_progress', or 'completed', got '{status}'"
                        ));
                    }

                    // Retry the load-modify-save up to 3 times to reduce the
                    // likelihood that two concurrent todo_write calls for the
                    // same session silently overwrite each other's change.
                    let mut last_err = String::new();
                    for attempt in 0..3u8 {
                        if attempt > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                20 * u64::from(attempt),
                            ))
                            .await;
                        }
                        let mut state =
                            store.load(&session_id).await.map_err(|e| e.to_string())?;
                        if let Some(t) = state.todos.iter_mut().find(|t| t.id == id) {
                            t.content = content.clone();
                            t.status = status.clone();
                        } else {
                            state.todos.push(TodoItem {
                                id: id.clone(),
                                content: content.clone(),
                                status: status.clone(),
                            });
                        }
                        match store.save(&session_id, &state).await {
                            Ok(()) => return Ok("OK".to_string()),
                            Err(e) => {
                                last_err = e.to_string();
                                tracing::warn!(
                                    session_id = %session_id,
                                    attempt,
                                    error = %last_err,
                                    "todo_write save failed, retrying"
                                );
                            }
                        }
                    }
                    Err(format!("todo_write failed after 3 attempts: {last_err}"))
                }
                "todo_read" => {
                    let state = store.load(&session_id).await.map_err(|e| e.to_string())?;
                    let active: Vec<&TodoItem> =
                        state.todos.iter().filter(|t| t.status != "completed").collect();
                    if active.is_empty() {
                        return Ok("No active todos.".to_string());
                    }
                    Ok(active
                        .iter()
                        .map(|t| format!("[{}] {} — {}", t.status, t.id, t.content))
                        .collect::<Vec<_>>()
                        .join("\n"))
                }
                other => Err(format!("NatsTodoTool: unknown operation '{other}'")),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "test-helpers")]
    mod with_mem_store {
        use super::*;
        use crate::session_store::mock::MemorySessionStore;

        fn tool(session_id: &str) -> NatsTodoTool<MemorySessionStore> {
            NatsTodoTool::new(session_id, MemorySessionStore::new())
        }

        #[tokio::test]
        async fn todo_write_creates_item() {
            let t = tool("s1");
            let r = t
                .call_tool(
                    "todo_write",
                    &serde_json::json!({"id":"t1","content":"do stuff","status":"pending"}),
                )
                .await
                .unwrap();
            assert_eq!(r, "OK");
            let state = t.store.load("s1").await.unwrap();
            assert_eq!(state.todos.len(), 1);
            assert_eq!(state.todos[0].id, "t1");
        }

        #[tokio::test]
        async fn todo_write_updates_existing_item() {
            let t = tool("s1");
            t.call_tool(
                "todo_write",
                &serde_json::json!({"id":"t1","content":"init","status":"pending"}),
            )
            .await
            .unwrap();
            t.call_tool(
                "todo_write",
                &serde_json::json!({"id":"t1","content":"updated","status":"in_progress"}),
            )
            .await
            .unwrap();
            let state = t.store.load("s1").await.unwrap();
            assert_eq!(state.todos.len(), 1);
            assert_eq!(state.todos[0].content, "updated");
            assert_eq!(state.todos[0].status, "in_progress");
        }

        #[tokio::test]
        async fn todo_write_rejects_invalid_status() {
            let t = tool("s1");
            let r = t
                .call_tool(
                    "todo_write",
                    &serde_json::json!({"id":"t1","content":"x","status":"bogus"}),
                )
                .await;
            assert!(r.is_err());
        }

        #[tokio::test]
        async fn todo_read_returns_active_todos() {
            let t = tool("s1");
            t.call_tool(
                "todo_write",
                &serde_json::json!({"id":"t1","content":"pending task","status":"pending"}),
            )
            .await
            .unwrap();
            t.call_tool(
                "todo_write",
                &serde_json::json!({"id":"t2","content":"done","status":"completed"}),
            )
            .await
            .unwrap();
            let r = t
                .call_tool("todo_read", &serde_json::json!({}))
                .await
                .unwrap();
            assert!(r.contains("t1"), "got: {r}");
            assert!(!r.contains("done"), "completed should be hidden, got: {r}");
        }

        #[tokio::test]
        async fn todo_read_empty() {
            let t = tool("s1");
            let r = t
                .call_tool("todo_read", &serde_json::json!({}))
                .await
                .unwrap();
            assert_eq!(r, "No active todos.");
        }

        #[tokio::test]
        async fn todo_write_missing_id_returns_error() {
            let t = tool("s1");
            let r = t
                .call_tool(
                    "todo_write",
                    &serde_json::json!({"content":"do stuff","status":"pending"}),
                )
                .await;
            assert!(r.is_err());
            assert!(r.unwrap_err().contains("id"), "error must mention 'id'");
        }

        #[tokio::test]
        async fn todo_write_missing_content_returns_error() {
            let t = tool("s1");
            let r = t
                .call_tool(
                    "todo_write",
                    &serde_json::json!({"id":"t1","status":"pending"}),
                )
                .await;
            assert!(r.is_err());
            assert!(r.unwrap_err().contains("content"), "error must mention 'content'");
        }

        #[tokio::test]
        async fn todo_write_missing_status_returns_error() {
            let t = tool("s1");
            let r = t
                .call_tool(
                    "todo_write",
                    &serde_json::json!({"id":"t1","content":"do stuff"}),
                )
                .await;
            assert!(r.is_err());
            assert!(r.unwrap_err().contains("status"), "error must mention 'status'");
        }

        #[tokio::test]
        async fn todo_read_when_all_completed_returns_no_active() {
            let t = tool("s1");
            t.call_tool(
                "todo_write",
                &serde_json::json!({"id":"t1","content":"done task","status":"completed"}),
            )
            .await
            .unwrap();
            t.call_tool(
                "todo_write",
                &serde_json::json!({"id":"t2","content":"also done","status":"completed"}),
            )
            .await
            .unwrap();
            let r = t.call_tool("todo_read", &serde_json::json!({})).await.unwrap();
            assert_eq!(r, "No active todos.");
        }

        #[tokio::test]
        async fn todo_write_invalid_status_error_mentions_received_value() {
            let t = tool("s1");
            let r = t
                .call_tool(
                    "todo_write",
                    &serde_json::json!({"id":"t1","content":"x","status":"invalid_state"}),
                )
                .await;
            let err = r.unwrap_err();
            assert!(err.contains("invalid_state"), "error must include the bad value, got: {err}");
        }

        #[tokio::test]
        async fn todo_read_output_format_includes_status_id_content() {
            let t = tool("s1");
            t.call_tool(
                "todo_write",
                &serde_json::json!({"id":"task-42","content":"write tests","status":"in_progress"}),
            )
            .await
            .unwrap();
            let r = t.call_tool("todo_read", &serde_json::json!({})).await.unwrap();
            assert!(r.contains("in_progress"), "output must contain status");
            assert!(r.contains("task-42"), "output must contain id");
            assert!(r.contains("write tests"), "output must contain content");
        }
    }

    /// The `other` branch (line 129) is reachable by calling `call_tool` directly
    /// with a name that is neither "todo_write" nor "todo_read".
    #[cfg(feature = "test-helpers")]
    #[tokio::test]
    async fn call_tool_unknown_operation_returns_error() {
        use crate::session_store::mock::MemorySessionStore;
        let tool = NatsTodoTool::new("s1", MemorySessionStore::new());
        let result = tool
            .call_tool("unknown_op", &serde_json::json!({}))
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("unknown operation"),
            "error must mention 'unknown operation', got: {err}"
        );
        assert!(
            err.contains("unknown_op"),
            "error must include the operation name, got: {err}"
        );
    }

    #[test]
    fn todo_write_def_name() {
        let def = NatsTodoTool::<crate::session_store::NatsSessionStore>::todo_write_def();
        assert_eq!(def.name, "todo_write");
    }

    #[test]
    fn todo_read_def_name() {
        let def = NatsTodoTool::<crate::session_store::NatsSessionStore>::todo_read_def();
        assert_eq!(def.name, "todo_read");
    }

    // ── into_dispatches ───────────────────────────────────────────────────────

    #[cfg(feature = "test-helpers")]
    mod into_dispatches_tests {
        use super::*;
        use crate::session_store::mock::MemorySessionStore;

        fn tool() -> NatsTodoTool<MemorySessionStore> {
            NatsTodoTool::new("s1", MemorySessionStore::new())
        }

        #[test]
        fn into_dispatches_returns_exactly_two_entries() {
            let dispatches = tool().into_dispatches();
            assert_eq!(dispatches.len(), 2);
        }

        #[test]
        fn into_dispatches_first_entry_is_todo_write() {
            let dispatches = tool().into_dispatches();
            let (server_name, tool_name, _) = &dispatches[0];
            assert_eq!(server_name, "todo_write");
            assert_eq!(tool_name, "todo_write");
        }

        #[test]
        fn into_dispatches_second_entry_is_todo_read() {
            let dispatches = tool().into_dispatches();
            let (server_name, tool_name, _) = &dispatches[1];
            assert_eq!(server_name, "todo_read");
            assert_eq!(tool_name, "todo_read");
        }

        #[test]
        fn into_dispatches_handlers_share_the_same_arc() {
            let dispatches = tool().into_dispatches();
            let (_, _, h0) = &dispatches[0];
            let (_, _, h1) = &dispatches[1];
            // Both Arc pointers must point to the same allocation.
            assert!(Arc::ptr_eq(h0, h1), "both dispatches must share the same Arc");
        }

        #[tokio::test]
        async fn into_dispatches_write_handler_creates_item() {
            let dispatches = tool().into_dispatches();
            let (_, _, handler) = &dispatches[0];
            let result = handler
                .call_tool(
                    "todo_write",
                    &serde_json::json!({"id":"t1","content":"task","status":"pending"}),
                )
                .await;
            assert_eq!(result.unwrap(), "OK");
        }

        #[tokio::test]
        async fn into_dispatches_read_handler_returns_todos() {
            let dispatches = tool().into_dispatches();
            let (_, _, write_handler) = &dispatches[0];
            let (_, _, read_handler) = &dispatches[1];

            write_handler
                .call_tool(
                    "todo_write",
                    &serde_json::json!({"id":"t1","content":"task","status":"pending"}),
                )
                .await
                .unwrap();

            let result = read_handler
                .call_tool("todo_read", &serde_json::json!({}))
                .await
                .unwrap();

            assert!(result.contains("t1"), "read must return written todo, got: {result}");
        }
    }

    // ── LOW-9: retry loop on save failure ────────────────────────────────────

    /// A SessionStore that fails `save()` the first `fail_count` times,
    /// then succeeds. `load()` always returns the last successfully saved state.
    #[derive(Clone)]
    struct FailNSaves {
        inner: std::sync::Arc<std::sync::Mutex<Option<crate::session_store::SessionState>>>,
        fails_left: std::sync::Arc<std::sync::Mutex<u8>>,
    }

    impl FailNSaves {
        fn new(fail_count: u8) -> Self {
            Self {
                inner: std::sync::Arc::new(std::sync::Mutex::new(None)),
                fails_left: std::sync::Arc::new(std::sync::Mutex::new(fail_count)),
            }
        }

        fn saved_state(&self) -> Option<crate::session_store::SessionState> {
            self.inner.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl super::SessionStore for FailNSaves {
        async fn load(&self, _: &str) -> anyhow::Result<crate::session_store::SessionState> {
            Ok(self.inner.lock().unwrap().clone().unwrap_or_default())
        }

        async fn save(&self, _: &str, state: &crate::session_store::SessionState) -> anyhow::Result<()> {
            let mut left = self.fails_left.lock().unwrap();
            if *left > 0 {
                *left -= 1;
                Err(anyhow::anyhow!("simulated KV revision conflict"))
            } else {
                *self.inner.lock().unwrap() = Some(state.clone());
                Ok(())
            }
        }

        async fn delete(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
        async fn list_ids(&self) -> anyhow::Result<Vec<String>> {
            Ok(vec![])
        }
        async fn list_children(&self, _: &str) -> anyhow::Result<Vec<String>> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn todo_write_succeeds_after_two_save_failures() {
        let store = FailNSaves::new(2); // first 2 saves fail, 3rd succeeds
        let tool = NatsTodoTool::new("session-1", store.clone());
        let result = tool
            .call_tool(
                "todo_write",
                &serde_json::json!({"id": "t1", "content": "test task", "status": "pending"}),
            )
            .await;
        assert_eq!(result, Ok("OK".to_string()), "should succeed after retries");
        let saved = store.saved_state().expect("state must be persisted");
        assert_eq!(saved.todos.len(), 1);
        assert_eq!(saved.todos[0].id, "t1");
    }

    #[tokio::test]
    async fn todo_write_fails_after_three_save_failures() {
        let store = FailNSaves::new(3); // all 3 attempts fail
        let tool = NatsTodoTool::new("session-1", store);
        let result = tool
            .call_tool(
                "todo_write",
                &serde_json::json!({"id": "t1", "content": "task", "status": "pending"}),
            )
            .await;
        let err = result.unwrap_err();
        assert!(
            err.contains("failed after 3 attempts"),
            "error should mention retry count, got: {err}"
        );
    }

    #[tokio::test]
    async fn todo_write_succeeds_immediately_when_no_failures() {
        let store = FailNSaves::new(0); // no failures
        let tool = NatsTodoTool::new("session-1", store.clone());
        let result = tool
            .call_tool(
                "todo_write",
                &serde_json::json!({"id": "t1", "content": "task", "status": "pending"}),
            )
            .await;
        assert_eq!(result, Ok("OK".to_string()));
        assert!(store.saved_state().is_some());
    }
}
