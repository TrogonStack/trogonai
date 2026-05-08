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

                    let mut state = store.load(&session_id).await.map_err(|e| e.to_string())?;
                    if let Some(t) = state.todos.iter_mut().find(|t| t.id == id) {
                        t.content = content;
                        t.status = status;
                    } else {
                        state.todos.push(TodoItem { id, content, status });
                    }
                    store.save(&session_id, &state).await.map_err(|e| e.to_string())?;
                    Ok("OK".to_string())
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
}
