use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod editor;
pub mod fs;
pub mod git;
pub mod web;

/// Anthropic tool definition sent in every request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// Set to `{"type":"ephemeral"}` on the last tool to enable prompt caching
    /// for the tool definitions block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<Value>,
}

/// Shared HTTP context available to every tool execution.
pub struct ToolContext {
    /// Base URL of the running `trogon-secret-proxy`.
    pub proxy_url: String,
    /// Working directory for the session — all filesystem tools resolve paths relative to this.
    pub cwd: String,
    /// Reusable HTTP client for web tools.
    pub http_client: reqwest::Client,
}

/// Build a [`ToolDef`] from name, description and a JSON Schema object.
pub fn tool_def(name: &str, description: &str, schema: Value) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: schema,
        cache_control: None,
    }
}

/// All tool definitions for the coding assistant tools.
/// The last entry has `cache_control` set so Anthropic caches the entire block.
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
    ];

    // Mark the last tool with cache_control so Anthropic caches the entire block.
    if let Some(last) = defs.last_mut() {
        last.cache_control = Some(serde_json::json!({"type": "ephemeral"}));
    }

    defs
}

/// Dispatch a tool call by name.
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

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error_string() {
        let ctx = test_ctx();
        let result = dispatch_tool(&ctx, "nonexistent_tool", &json!({})).await;
        assert!(result.contains("Unknown tool"));
    }
}
