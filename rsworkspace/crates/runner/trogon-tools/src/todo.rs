use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use crate::ToolContext;

const TODO_FILE: &str = ".trogon/todos.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TodoItem {
    id: String,
    content: String,
    status: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TodoStore {
    todos: Vec<TodoItem>,
}

async fn load_store(cwd: &str) -> TodoStore {
    let path = std::path::Path::new(cwd).join(TODO_FILE);
    match tokio::fs::read_to_string(&path).await {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => TodoStore::default(),
    }
}

#[derive(Debug, Deserialize)]
struct TodoWriteItemWire {
    id: String,
    content: String,
    status: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TodoWriteInput {
    Batch {
        todos: Vec<TodoWriteItemWire>,
    },
    Single {
        id: String,
        content: String,
        status: String,
    },
}

fn parse_todo_write_input(input: &Value) -> Result<TodoWriteInput, String> {
    if input.get("todos").is_some() {
        serde_json::from_value(input.clone()).map_err(|e| format!("Error: invalid todos batch: {e}"))
    } else {
        serde_json::from_value(input.clone()).map_err(|e| format!("Error: {e}"))
    }
}

fn validate_status(status: &str) -> Result<(), String> {
    if matches!(status, "pending" | "in_progress" | "completed") {
        Ok(())
    } else {
        Err(format!(
            "Error: status must be 'pending', 'in_progress', or 'completed', got '{status}'"
        ))
    }
}

fn upsert_todo(store: &mut TodoStore, item: TodoItem) {
    if let Some(existing) = store.todos.iter_mut().find(|t| t.id == item.id) {
        existing.content = item.content;
        existing.status = item.status;
    } else {
        store.todos.push(item);
    }
}

async fn save_store(cwd: &str, store: &TodoStore) -> Result<(), String> {
    let dir = std::path::Path::new(cwd).join(".trogon");
    tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
    let path = dir.join("todos.json");
    // MED-12: unique temp name so concurrent saves don't clobber each other.
    let tmp = crate::fs::unique_tmp_path(&path);
    let json = serde_json::to_vec_pretty(store).map_err(|e| e.to_string())?;
    let mut file = tokio::fs::File::create(&tmp).await.map_err(|e| e.to_string())?;
    file.write_all(&json).await.map_err(|e| e.to_string())?;
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    tokio::fs::rename(&tmp, &path).await.map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn todo_write(ctx: &ToolContext, input: &Value) -> String {
    let parsed = match parse_todo_write_input(input) {
        Ok(parsed) => parsed,
        Err(message) => return message,
    };

    match parsed {
        TodoWriteInput::Single { id, content, status } => {
            if let Err(message) = validate_status(&status) {
                return message;
            }

            let mut store = load_store(&ctx.cwd).await;
            upsert_todo(&mut store, TodoItem { id, content, status });

            match save_store(&ctx.cwd, &store).await {
                Ok(()) => "OK".to_string(),
                Err(e) => format!("Error: {e}"),
            }
        }
        TodoWriteInput::Batch { todos } => {
            let mut items = Vec::with_capacity(todos.len());
            for item in todos {
                if let Err(message) = validate_status(&item.status) {
                    return message;
                }
                items.push(TodoItem {
                    id: item.id,
                    content: item.content,
                    status: item.status,
                });
            }

            let store = TodoStore { todos: items };
            match save_store(&ctx.cwd, &store).await {
                Ok(()) => "OK".to_string(),
                Err(e) => format!("Error: {e}"),
            }
        }
    }
}

pub async fn todo_read(ctx: &ToolContext, _input: &Value) -> String {
    let store = load_store(&ctx.cwd).await;
    let active: Vec<&TodoItem> = store.todos.iter().filter(|t| t.status != "completed").collect();

    if active.is_empty() {
        return "No active todos.".to_string();
    }

    active
        .iter()
        .map(|t| format!("[{}] {} — {}", t.status, t.id, t.content))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn ctx(dir: &TempDir) -> ToolContext {
        ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
            web_search_api_key: None,
            web_search_endpoint: None,
        }
    }

    #[tokio::test]
    async fn todo_write_creates_item() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let input = serde_json::json!({"id": "t1", "content": "do stuff", "status": "pending"});
        let result = todo_write(&ctx, &input).await;
        assert_eq!(result, "OK");

        let store = load_store(&ctx.cwd).await;
        assert_eq!(store.todos.len(), 1);
        assert_eq!(store.todos[0].id, "t1");
        assert_eq!(store.todos[0].status, "pending");
    }

    #[tokio::test]
    async fn todo_write_updates_existing_item() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let input = serde_json::json!({"id": "t1", "content": "initial", "status": "pending"});
        todo_write(&ctx, &input).await;
        let input2 = serde_json::json!({"id": "t1", "content": "updated", "status": "in_progress"});
        let result = todo_write(&ctx, &input2).await;
        assert_eq!(result, "OK");

        let store = load_store(&ctx.cwd).await;
        assert_eq!(store.todos.len(), 1);
        assert_eq!(store.todos[0].content, "updated");
        assert_eq!(store.todos[0].status, "in_progress");
    }

    #[tokio::test]
    async fn todo_write_rejects_invalid_status() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let input = serde_json::json!({"id": "t1", "content": "x", "status": "unknown"});
        let result = todo_write(&ctx, &input).await;
        assert!(result.starts_with("Error:"), "got: {result}");
    }

    #[tokio::test]
    async fn todo_read_returns_active_todos() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        todo_write(
            &ctx,
            &serde_json::json!({"id": "t1", "content": "pending task", "status": "pending"}),
        )
        .await;
        todo_write(
            &ctx,
            &serde_json::json!({"id": "t2", "content": "in progress", "status": "in_progress"}),
        )
        .await;
        todo_write(
            &ctx,
            &serde_json::json!({"id": "t3", "content": "done", "status": "completed"}),
        )
        .await;

        let result = todo_read(&ctx, &serde_json::json!({})).await;
        assert!(result.contains("t1"), "got: {result}");
        assert!(result.contains("t2"), "got: {result}");
        assert!(!result.contains("done"), "completed should not appear, got: {result}");
    }

    #[tokio::test]
    async fn todo_read_empty_returns_no_active_message() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = todo_read(&ctx, &serde_json::json!({})).await;
        assert_eq!(result, "No active todos.");
    }

    #[tokio::test]
    async fn todo_write_missing_id_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let input = serde_json::json!({"content": "x", "status": "pending"});
        let result = todo_write(&ctx, &input).await;
        assert!(result.starts_with("Error:"), "got: {result}");
    }

    #[tokio::test]
    async fn todo_write_batch_replaces_list() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        todo_write(
            &ctx,
            &serde_json::json!({"id": "old", "content": "legacy", "status": "pending"}),
        )
        .await;

        let input = serde_json::json!({
            "todos": [
                {"id": "t1", "content": "first", "status": "pending"},
                {"id": "t2", "content": "second", "status": "in_progress"}
            ]
        });
        let result = todo_write(&ctx, &input).await;
        assert_eq!(result, "OK");

        let store = load_store(&ctx.cwd).await;
        assert_eq!(store.todos.len(), 2);
        assert_eq!(store.todos[0].id, "t1");
        assert_eq!(store.todos[1].id, "t2");
        assert!(
            !store.todos.iter().any(|t| t.id == "old"),
            "batch write should replace the previous list"
        );
    }

    #[tokio::test]
    async fn todo_write_single_shape_still_works() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let input = serde_json::json!({"id": "t1", "content": "do stuff", "status": "pending"});
        let result = todo_write(&ctx, &input).await;
        assert_eq!(result, "OK");

        let store = load_store(&ctx.cwd).await;
        assert_eq!(store.todos.len(), 1);
        assert_eq!(store.todos[0].id, "t1");
        assert_eq!(store.todos[0].content, "do stuff");
        assert_eq!(store.todos[0].status, "pending");
    }
}
