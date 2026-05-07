use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use super::ToolContext;

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

async fn save_store(cwd: &str, store: &TodoStore) -> Result<(), String> {
    let dir = std::path::Path::new(cwd).join(".trogon");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let path = dir.join("todos.json");
    let tmp = path.with_extension("tmp");
    let json = serde_json::to_vec_pretty(store).map_err(|e| e.to_string())?;
    let mut file = tokio::fs::File::create(&tmp)
        .await
        .map_err(|e| e.to_string())?;
    file.write_all(&json).await.map_err(|e| e.to_string())?;
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Write (create or update) a todo item.
pub async fn todo_write(ctx: &ToolContext, input: &Value) -> String {
    let id = match input["id"].as_str() {
        Some(s) => s.to_string(),
        None => return "Error: missing field 'id'".to_string(),
    };
    let content = match input["content"].as_str() {
        Some(s) => s.to_string(),
        None => return "Error: missing field 'content'".to_string(),
    };
    let status = match input["status"].as_str() {
        Some(s) => s.to_string(),
        None => return "Error: missing field 'status'".to_string(),
    };
    if !matches!(status.as_str(), "pending" | "in_progress" | "completed") {
        return format!(
            "Error: status must be 'pending', 'in_progress', or 'completed', got '{status}'"
        );
    }

    let mut store = load_store(&ctx.cwd).await;
    if let Some(existing) = store.todos.iter_mut().find(|t| t.id == id) {
        existing.content = content;
        existing.status = status;
    } else {
        store.todos.push(TodoItem { id, content, status });
    }

    match save_store(&ctx.cwd, &store).await {
        Ok(()) => "OK".to_string(),
        Err(e) => format!("Error: {e}"),
    }
}

/// Read active todos (status != completed).
pub async fn todo_read(ctx: &ToolContext, _input: &Value) -> String {
    let store = load_store(&ctx.cwd).await;
    let active: Vec<&TodoItem> = store
        .todos
        .iter()
        .filter(|t| t.status != "completed")
        .collect();

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
        todo_write(&ctx, &serde_json::json!({"id": "t1", "content": "pending task", "status": "pending"})).await;
        todo_write(&ctx, &serde_json::json!({"id": "t2", "content": "in progress", "status": "in_progress"})).await;
        todo_write(&ctx, &serde_json::json!({"id": "t3", "content": "done", "status": "completed"})).await;

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
}
