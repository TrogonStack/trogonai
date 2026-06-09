//! Cross-runner contract for editor tools.
//!
//! ACP, OpenRouter, and xAI runners all register tools from
//! `trogon_tools::all_tool_defs()` and dispatch via `trogon_tools::dispatch_tool`.
//! These tests assert the shared definitions and dispatch behavior that every
//! runner must expose identically.

use serde_json::json;
use tempfile::TempDir;
use trogon_tools::{ToolContext, all_tool_defs, dispatch_tool};

fn ctx(dir: &TempDir) -> ToolContext {
    ToolContext {
        proxy_url: String::new(),
        cwd: dir.path().to_string_lossy().into_owned(),
        http_client: reqwest::Client::new(),
        web_search_api_key: None,
        web_search_endpoint: None,
    }
}

fn tool_def_by_name(name: &str) -> trogon_tools::ToolDef {
    all_tool_defs()
        .into_iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("{name} must be in all_tool_defs"))
}

#[test]
fn shared_str_replace_def_includes_replace_all() {
    let def = tool_def_by_name("str_replace");
    assert!(
        def.input_schema["properties"].get("replace_all").is_some(),
        "str_replace schema must include replace_all for all runners"
    );
}

#[test]
fn shared_multi_edit_def_present_with_edits_array() {
    let def = tool_def_by_name("multi_edit");
    let edits = def
        .input_schema["properties"]
        .get("edits")
        .expect("multi_edit schema must include edits array");
    assert_eq!(edits["type"], "array");
    assert!(
        edits["items"]["properties"].get("replace_all").is_some(),
        "multi_edit edit items must support replace_all"
    );
}

#[tokio::test]
async fn shared_dispatch_str_replace_replace_all() {
    let dir = TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("f.txt"), "a\na\n")
        .await
        .unwrap();

    let result = dispatch_tool(
        &ctx(&dir),
        "str_replace",
        &json!({"path": "f.txt", "old_str": "a", "new_str": "b", "replace_all": true}),
    )
    .await;

    assert!(!result.starts_with("Error"), "replace_all dispatch must succeed; got: {result}");
    let content = tokio::fs::read_to_string(dir.path().join("f.txt"))
        .await
        .unwrap();
    assert_eq!(content, "b\nb\n");
}

#[tokio::test]
async fn shared_dispatch_multi_edit_sequence() {
    let dir = TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("f.txt"), "one two\n")
        .await
        .unwrap();

    let result = dispatch_tool(
        &ctx(&dir),
        "multi_edit",
        &json!({
            "path": "f.txt",
            "edits": [
                {"old_str": "one", "new_str": "1"},
                {"old_str": "two", "new_str": "2"}
            ]
        }),
    )
    .await;

    assert!(!result.starts_with("Error"), "multi_edit dispatch must succeed; got: {result}");
    let content = tokio::fs::read_to_string(dir.path().join("f.txt"))
        .await
        .unwrap();
    assert_eq!(content, "1 2\n");
}

#[tokio::test]
async fn shared_dispatch_multi_edit_atomic_failure() {
    let dir = TempDir::new().unwrap();
    tokio::fs::write(dir.path().join("f.txt"), "unchanged\n")
        .await
        .unwrap();

    let result = dispatch_tool(
        &ctx(&dir),
        "multi_edit",
        &json!({
            "path": "f.txt",
            "edits": [
                {"old_str": "unchanged", "new_str": "changed"},
                {"old_str": "missing", "new_str": "x"}
            ]
        }),
    )
    .await;

    assert!(result.contains("No changes were written"), "got: {result}");
    let content = tokio::fs::read_to_string(dir.path().join("f.txt"))
        .await
        .unwrap();
    assert_eq!(content, "unchanged\n");
}
