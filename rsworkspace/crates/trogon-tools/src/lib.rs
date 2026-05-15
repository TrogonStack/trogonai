use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod editor;
pub mod fs;
pub mod git;
pub mod search;
pub mod todo;
pub mod types;
pub mod web;

pub use types::{
    ContentBlock, ElicitationProvider, ImageSource, Message, PermissionChecker, ToolResult,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<Value>,
}

pub struct ToolContext {
    /// Base URL of the running secret proxy (used by fetch_url).
    pub proxy_url: String,
    /// Working directory — all filesystem tools resolve paths relative to this.
    pub cwd: String,
    /// Reusable HTTP client for web tools.
    pub http_client: reqwest::Client,
}

pub fn tool_def(name: &str, description: &str, schema: Value) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: schema,
        cache_control: None,
    }
}

pub fn all_tool_defs() -> Vec<ToolDef> {
    use serde_json::json;
    let mut defs = vec![
        tool_def(
            "read_file",
            "Read the contents of a file. Returns content with line numbers. Supports optional offset and limit for large files.",
            json!({
                "type": "object",
                "properties": {
                    "path":   { "type": "string", "description": "Path to the file, relative to the working directory" },
                    "offset": { "type": "integer", "description": "Line number to start reading from (0-indexed)" },
                    "limit":  { "type": "integer", "description": "Maximum number of lines to return" }
                },
                "required": ["path"]
            }),
        ),
        tool_def(
            "write_file",
            "Write content to a file. Creates the file and any missing parent directories. Overwrites existing content.",
            json!({
                "type": "object",
                "properties": {
                    "path":    { "type": "string", "description": "Path to the file, relative to the working directory" },
                    "content": { "type": "string", "description": "Content to write" }
                },
                "required": ["path", "content"]
            }),
        ),
        tool_def(
            "list_dir",
            "List the contents of a directory as a tree. Respects .gitignore. Limits output to 500 entries.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path relative to working directory (default: '.')" }
                },
                "required": []
            }),
        ),
        tool_def(
            "glob",
            "Find files matching a glob pattern. Respects .gitignore. Returns paths relative to the working directory.",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern, e.g. 'src/**/*.rs' or '**/*.test.ts'" },
                    "path":    { "type": "string", "description": "Base directory to search from (default: '.')" }
                },
                "required": ["pattern"]
            }),
        ),
        tool_def(
            "str_replace",
            "Replace an exact string in a file. The old_str must appear exactly once — if it appears 0 or more than once, the edit is rejected. Returns a diff of the changes.",
            json!({
                "type": "object",
                "properties": {
                    "path":    { "type": "string", "description": "Path to the file, relative to the working directory" },
                    "old_str": { "type": "string", "description": "Exact string to replace — must appear exactly once in the file" },
                    "new_str": { "type": "string", "description": "String to replace it with" }
                },
                "required": ["path", "old_str", "new_str"]
            }),
        ),
        tool_def(
            "git_status",
            "Run `git status --short` in the working directory.",
            json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        ),
        tool_def(
            "git_diff",
            "Run `git diff` in the working directory. Output is truncated at 4KB.",
            json!({
                "type": "object",
                "properties": {
                    "args": { "type": "string", "description": "Optional extra arguments, e.g. '--staged' or a file path" }
                },
                "required": []
            }),
        ),
        tool_def(
            "git_log",
            "Run `git log --oneline -20` in the working directory.",
            json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        ),
        tool_def(
            "fetch_url",
            "Fetch the content of a URL. HTML is converted to plain text by default. Response is truncated at 8KB.",
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to fetch" },
                    "raw": { "type": "boolean", "description": "If true, return raw response body without HTML conversion (default: false)" }
                },
                "required": ["url"]
            }),
        ),
        tool_def(
            "notebook_edit",
            "Edit a cell in a Jupyter notebook (.ipynb file) by index.",
            json!({
                "type": "object",
                "properties": {
                    "path":       { "type": "string",  "description": "Path to the .ipynb file, relative to the working directory" },
                    "cell_index": { "type": "integer", "description": "Zero-based index of the cell to edit" },
                    "content":    { "type": "string",  "description": "New content for the cell" },
                    "cell_type":  { "type": "string",  "description": "Cell type: 'code' or 'markdown' (optional, keeps existing type if omitted)" }
                },
                "required": ["path", "cell_index", "content"]
            }),
        ),
        tool_def(
            "search_files",
            "Search for a literal string pattern across files in the working directory. Respects .gitignore. Returns matching lines with file:line format, truncated at 100 matches.",
            json!({
                "type": "object",
                "properties": {
                    "pattern":          { "type": "string",  "description": "Literal string to search for" },
                    "path":             { "type": "string",  "description": "Directory to search in, relative to working directory (default: '.')" },
                    "case_insensitive": { "type": "boolean", "description": "If true, search is case-insensitive (default: false)" }
                },
                "required": ["pattern"]
            }),
        ),
        tool_def(
            "todo_write",
            "Create or update a todo item. Status must be 'pending', 'in_progress', or 'completed'.",
            json!({
                "type": "object",
                "properties": {
                    "id":      { "type": "string", "description": "Unique identifier for the todo" },
                    "content": { "type": "string", "description": "Description of the task" },
                    "status":  { "type": "string", "description": "One of: pending, in_progress, completed" }
                },
                "required": ["id", "content", "status"]
            }),
        ),
        tool_def(
            "todo_read",
            "List all active (non-completed) todos for the current session.",
            json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        ),
    ];

    if let Some(last) = defs.last_mut() {
        last.cache_control = Some(serde_json::json!({"type": "ephemeral"}));
    }

    defs
}

pub async fn dispatch_tool(ctx: &ToolContext, name: &str, input: &Value) -> String {
    match name {
        "read_file"     => fs::read_file(ctx, input).await,
        "write_file"    => fs::write_file(ctx, input).await,
        "list_dir"      => fs::list_dir(ctx, input).await,
        "glob"          => fs::glob_files(ctx, input).await,
        "str_replace"   => editor::str_replace(ctx, input).await,
        "git_status"    => git::status(ctx, input).await,
        "git_diff"      => git::diff(ctx, input).await,
        "git_log"       => git::log(ctx, input).await,
        "fetch_url"     => web::fetch_url(ctx, input).await,
        "notebook_edit" => fs::notebook_edit(ctx, input).await,
        "search_files"  => search::search_files(ctx, input).await,
        "todo_write"    => todo::todo_write(ctx, input).await,
        "todo_read"     => todo::todo_read(ctx, input).await,
        _               => format!("Unknown tool: {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_ctx() -> ToolContext {
        ToolContext {
            proxy_url: "http://localhost:8080".to_string(),
            cwd: std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .to_string(),
            http_client: reqwest::Client::new(),
        }
    }

    #[test]
    fn tool_def_stores_fields() {
        let t = tool_def(
            "my_tool",
            "Does something",
            json!({"type": "object", "properties": {}}),
        );
        assert_eq!(t.name, "my_tool");
        assert_eq!(t.description, "Does something");
    }

    #[test]
    fn all_tool_defs_last_has_cache_control() {
        let defs = all_tool_defs();
        assert!(!defs.is_empty());
        assert!(defs.last().unwrap().cache_control.is_some());
        for d in &defs[..defs.len() - 1] {
            assert!(d.cache_control.is_none());
        }
    }

    #[test]
    fn all_tool_defs_contains_todo_tools() {
        let defs = all_tool_defs();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"todo_write"), "missing todo_write, got: {names:?}");
        assert!(names.contains(&"todo_read"), "missing todo_read, got: {names:?}");
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error_string() {
        let ctx = test_ctx();
        let result = dispatch_tool(&ctx, "nonexistent_tool", &json!({})).await;
        assert!(result.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn dispatch_routes_read_file_to_fs() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("hi.txt"), "hello").await.unwrap();
        let ctx = ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
        };
        let result = dispatch_tool(&ctx, "read_file", &json!({"path": "hi.txt"})).await;
        assert!(result.contains("hello"), "got: {result}");
    }

    #[tokio::test]
    async fn dispatch_routes_write_file_to_fs() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let ctx = ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
        };
        let result = dispatch_tool(&ctx, "write_file", &json!({"path": "out.txt", "content": "data"})).await;
        assert_eq!(result, "OK");
        let content = tokio::fs::read_to_string(dir.path().join("out.txt")).await.unwrap();
        assert_eq!(content, "data");
    }

    #[tokio::test]
    async fn dispatch_routes_list_dir_to_fs() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("a.rs"), "").await.unwrap();
        let ctx = ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
        };
        let result = dispatch_tool(&ctx, "list_dir", &json!({})).await;
        assert!(result.contains("a.rs"), "got: {result}");
    }

    #[tokio::test]
    async fn dispatch_routes_glob_to_fs() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("foo.rs"), "").await.unwrap();
        let ctx = ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
        };
        let result = dispatch_tool(&ctx, "glob", &json!({"pattern": "*.rs"})).await;
        assert!(result.contains("foo.rs"), "got: {result}");
    }

    #[tokio::test]
    async fn dispatch_routes_str_replace_to_editor() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("f.txt"), "aaa\nbbb\n").await.unwrap();
        let ctx = ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
        };
        let result = dispatch_tool(&ctx, "str_replace", &json!({"path": "f.txt", "old_str": "aaa", "new_str": "zzz"})).await;
        assert!(!result.starts_with("Error"), "got: {result}");
        let content = tokio::fs::read_to_string(dir.path().join("f.txt")).await.unwrap();
        assert!(content.contains("zzz"));
    }

    #[tokio::test]
    async fn dispatch_routes_git_status_to_git() {
        let ctx = test_ctx();
        let result = dispatch_tool(&ctx, "git_status", &json!({})).await;
        assert!(!result.starts_with("Error running git"), "got: {result}");
    }

    #[tokio::test]
    async fn dispatch_routes_git_diff_to_git() {
        let ctx = test_ctx();
        let result = dispatch_tool(&ctx, "git_diff", &json!({})).await;
        assert!(!result.starts_with("Error running git"), "got: {result}");
    }

    #[tokio::test]
    async fn dispatch_routes_git_log_to_git() {
        let ctx = test_ctx();
        let result = dispatch_tool(&ctx, "git_log", &json!({})).await;
        assert!(!result.starts_with("Error running git"), "got: {result}");
    }

    #[tokio::test]
    async fn dispatch_routes_fetch_url_to_web() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/hi");
            then.status(200).body("dispatched");
        });
        let ctx = ToolContext {
            proxy_url: String::new(),
            cwd: ".".to_string(),
            http_client: reqwest::Client::new(),
        };
        let result = dispatch_tool(&ctx, "fetch_url", &json!({"url": server.url("/hi"), "raw": true})).await;
        assert!(result.contains("dispatched"), "got: {result}");
    }

    #[tokio::test]
    async fn dispatch_routes_notebook_edit_to_fs() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let nb = serde_json::json!({
            "nbformat": 4,
            "cells": [{"cell_type": "code", "source": ["old"], "metadata": {}, "outputs": [], "execution_count": null}]
        });
        tokio::fs::write(dir.path().join("nb.ipynb"), serde_json::to_string(&nb).unwrap()).await.unwrap();
        let ctx = ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
        };
        let result = dispatch_tool(&ctx, "notebook_edit", &json!({"path": "nb.ipynb", "cell_index": 0, "content": "new"})).await;
        assert_eq!(result, "OK");
    }

    #[tokio::test]
    async fn dispatch_routes_todo_write_to_todo() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let ctx = ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
        };
        let result = dispatch_tool(
            &ctx,
            "todo_write",
            &json!({"id": "t1", "content": "do stuff", "status": "pending"}),
        )
        .await;
        assert_eq!(result, "OK");
    }

    #[tokio::test]
    async fn dispatch_routes_todo_read_to_todo() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let ctx = ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
        };
        dispatch_tool(
            &ctx,
            "todo_write",
            &json!({"id": "t1", "content": "pending task", "status": "pending"}),
        )
        .await;
        let result = dispatch_tool(&ctx, "todo_read", &json!({})).await;
        assert!(result.contains("t1"), "got: {result}");
    }

    #[tokio::test]
    async fn dispatch_routes_search_files_to_search() {
        use std::fs as stdfs;
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        stdfs::write(dir.path().join("foo.rs"), "fn needle() {}\n").unwrap();
        let ctx = ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
        };
        let result = dispatch_tool(&ctx, "search_files", &json!({"pattern": "needle"})).await;
        assert!(result.contains("foo.rs"), "got: {result}");
    }
}
