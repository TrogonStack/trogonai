use serde_json::Value;

use crate::tools::ToolContext;

/// Resolve a user-supplied path against `cwd`, rejecting traversal outside it.
/// Does not require the path to exist on disk.
pub(crate) fn resolve_path(
    cwd: &str,
    path: &str,
) -> Result<std::path::PathBuf, String> {
    use std::path::{Component, PathBuf};

    let joined = std::path::Path::new(cwd).join(path);
    let mut normalized = PathBuf::new();

    for component in joined.components() {
        match component {
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err("path is outside the working directory".to_string());
                }
            }
            Component::CurDir => {}
            c => normalized.push(c),
        }
    }

    let cwd_norm: PathBuf = std::path::Path::new(cwd)
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();

    if !normalized.starts_with(&cwd_norm) {
        return Err("path is outside the working directory".to_string());
    }

    Ok(normalized)
}

pub async fn read_file(ctx: &ToolContext, input: &Value) -> String {
    let path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return "Error: missing required parameter 'path'".to_string(),
    };
    let offset = input
        .get("offset")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let limit = input
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);

    let full_path = match resolve_path(&ctx.cwd, path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {e}"),
    };

    let content = match tokio::fs::read_to_string(&full_path).await {
        Ok(c) => c,
        Err(e) => return format!("Error reading {path}: {e}"),
    };

    let lines: Vec<&str> = content.lines().collect();
    let start = offset.unwrap_or(0);

    if start > lines.len() {
        return format!(
            "Error: offset {start} exceeds file length ({} lines)",
            lines.len()
        );
    }

    let end = limit
        .map(|l| (start + l).min(lines.len()))
        .unwrap_or(lines.len());

    lines[start..end]
        .iter()
        .enumerate()
        .map(|(i, line)| format!("{:>6}\t{line}", start + i + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn write_file(ctx: &ToolContext, input: &Value) -> String {
    let path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return "Error: missing required parameter 'path'".to_string(),
    };
    let content = match input.get("content").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return "Error: missing required parameter 'content'".to_string(),
    };

    let full_path = match resolve_path(&ctx.cwd, path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {e}"),
    };

    if let Some(parent) = full_path.parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await {
        return format!("Error creating directories: {e}");
    }

    let tmp = full_path.with_extension(format!(
        "{}.tmp",
        full_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("tmp")
    ));
    if let Err(e) = tokio::fs::write(&tmp, content).await {
        return format!("Error writing file: {e}");
    }
    if let Err(e) = tokio::fs::rename(&tmp, &full_path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return format!("Error saving file: {e}");
    }

    "OK".to_string()
}

pub async fn list_dir(ctx: &ToolContext, input: &Value) -> String {
    let path = input
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");

    let full_path = match resolve_path(&ctx.cwd, path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {e}"),
    };

    let mut entries: Vec<String> = Vec::new();

    let walker = ignore::WalkBuilder::new(&full_path)
        .hidden(false)
        .build();

    for entry in walker {
        let e = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let rel = match e.path().strip_prefix(&full_path) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let depth = rel.components().count().saturating_sub(1);
        let indent = "  ".repeat(depth);
        let name = e.file_name().to_string_lossy();
        let suffix = if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            "/"
        } else {
            ""
        };
        entries.push(format!("{indent}{name}{suffix}"));

        if entries.len() >= 500 {
            entries.push("... (truncated at 500 entries)".to_string());
            break;
        }
    }

    if entries.is_empty() {
        return "(empty directory)".to_string();
    }

    entries.join("\n")
}

pub async fn glob_files(ctx: &ToolContext, input: &Value) -> String {
    let pattern = match input.get("pattern").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return "Error: missing required parameter 'pattern'".to_string(),
    };
    let base = input
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");

    let full_base = match resolve_path(&ctx.cwd, base) {
        Ok(p) => p,
        Err(e) => return format!("Error: {e}"),
    };

    let matcher = match globset::Glob::new(pattern) {
        Ok(g) => g.compile_matcher(),
        Err(e) => return format!("Error: invalid glob pattern: {e}"),
    };

    let mut matches: Vec<String> = Vec::new();

    let walker = ignore::WalkBuilder::new(&full_base)
        .hidden(false)
        .build();

    for e in walker.flatten() {
        if e.file_type().map(|t| t.is_file()).unwrap_or(false)
            && let Ok(rel) = e.path().strip_prefix(&full_base)
            && matcher.is_match(rel)
        {
            matches.push(rel.to_string_lossy().into_owned());
        }
    }

    if matches.is_empty() {
        return "No files found".to_string();
    }

    matches.join("\n")
}

pub async fn notebook_edit(ctx: &ToolContext, input: &Value) -> String {
    let path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return "Error: missing required parameter 'path'".to_string(),
    };
    let cell_index = match input.get("cell_index").and_then(|v| v.as_u64()) {
        Some(i) => i as usize,
        None => return "Error: missing required parameter 'cell_index'".to_string(),
    };
    let content = match input.get("content").and_then(|v| v.as_str()) {
        Some(c) => c,
        None => return "Error: missing required parameter 'content'".to_string(),
    };
    let cell_type = input.get("cell_type").and_then(|v| v.as_str());

    let full_path = match resolve_path(&ctx.cwd, path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {e}"),
    };

    let raw = match tokio::fs::read_to_string(&full_path).await {
        Ok(c) => c,
        Err(e) => return format!("Error reading notebook: {e}"),
    };

    let mut notebook: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return format!("Error parsing notebook JSON: {e}"),
    };

    let cells = match notebook.get_mut("cells").and_then(|v| v.as_array_mut()) {
        Some(c) => c,
        None => return "Error: notebook has no 'cells' array".to_string(),
    };

    if cell_index >= cells.len() {
        return format!(
            "Error: cell_index {cell_index} out of range (notebook has {} cells)",
            cells.len()
        );
    }

    let cell = &mut cells[cell_index];
    let source_lines: Vec<String> = content
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i == content.lines().count() - 1 {
                line.to_string()
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    cell["source"] = serde_json::json!(source_lines);

    if let Some(ct) = cell_type {
        cell["cell_type"] = serde_json::json!(ct);
    }

    let updated = match serde_json::to_string_pretty(&notebook) {
        Ok(s) => s,
        Err(e) => return format!("Error serializing notebook: {e}"),
    };

    if let Err(e) = tokio::fs::write(&full_path, updated).await {
        return format!("Error writing notebook: {e}");
    }

    "OK".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn ctx(dir: &TempDir) -> ToolContext {
        ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
        }
    }

    // ── resolve_path ──────────────────────────────────────────────────────────

    #[test]
    fn resolve_path_normal_relative() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let result = resolve_path(&cwd, "src/main.rs").unwrap();
        assert_eq!(result, dir.path().join("src/main.rs"));
    }

    #[test]
    fn resolve_path_dot_prefix() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let result = resolve_path(&cwd, "./foo.txt").unwrap();
        assert_eq!(result, dir.path().join("foo.txt"));
    }

    #[test]
    fn resolve_path_dotdot_stays_within_cwd() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let result = resolve_path(&cwd, "a/../b.txt").unwrap();
        assert_eq!(result, dir.path().join("b.txt"));
    }

    #[test]
    fn resolve_path_dotdot_escapes_cwd_is_rejected() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let err = resolve_path(&cwd, "../../etc/passwd").unwrap_err();
        assert!(err.contains("outside"), "got: {err}");
    }

    #[test]
    fn resolve_path_absolute_within_cwd_allowed() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let abs = dir.path().join("sub/file.txt").to_string_lossy().into_owned();
        let result = resolve_path(&cwd, &abs).unwrap();
        assert_eq!(result, dir.path().join("sub/file.txt"));
    }

    #[test]
    fn resolve_path_absolute_outside_cwd_rejected() {
        let dir = TempDir::new().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let err = resolve_path(&cwd, "/etc/passwd").unwrap_err();
        assert!(err.contains("outside"), "got: {err}");
    }

    // ── read_file ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn read_file_missing_path_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = read_file(&ctx, &json!({})).await;
        assert!(result.contains("missing required parameter 'path'"), "got: {result}");
    }

    #[tokio::test]
    async fn read_file_offset_beyond_file_length_returns_error() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("f.txt"), "a\nb\nc").await.unwrap();
        let ctx = ctx(&dir);
        let result = read_file(&ctx, &json!({"path": "f.txt", "offset": 100})).await;
        assert!(result.contains("Error"), "got: {result}");
        assert!(result.contains("exceeds"), "got: {result}");
    }

    #[tokio::test]
    async fn read_file_returns_numbered_lines() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("hello.txt"), "line1\nline2\nline3")
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = read_file(&ctx, &json!({"path": "hello.txt"})).await;
        assert!(result.contains("1\tline1"));
        assert!(result.contains("2\tline2"));
        assert!(result.contains("3\tline3"));
    }

    #[tokio::test]
    async fn read_file_with_offset_and_limit() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("f.txt"), "a\nb\nc\nd\ne")
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = read_file(&ctx, &json!({"path": "f.txt", "offset": 1, "limit": 2})).await;
        assert!(result.contains("2\tb"));
        assert!(result.contains("3\tc"));
        assert!(!result.contains("1\ta"));
        assert!(!result.contains("4\td"));
    }

    #[tokio::test]
    async fn read_file_missing_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = read_file(&ctx, &json!({"path": "no_such_file.txt"})).await;
        assert!(result.starts_with("Error"));
    }

    #[tokio::test]
    async fn read_file_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = read_file(&ctx, &json!({"path": "../../etc/passwd"})).await;
        assert!(result.contains("Error"));
    }

    #[tokio::test]
    async fn write_file_missing_path_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = write_file(&ctx, &json!({"content": "x"})).await;
        assert!(result.contains("missing required parameter 'path'"), "got: {result}");
    }

    #[tokio::test]
    async fn write_file_missing_content_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = write_file(&ctx, &json!({"path": "f.txt"})).await;
        assert!(result.contains("missing required parameter 'content'"), "got: {result}");
    }

    #[tokio::test]
    async fn write_file_creates_and_reads_back() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = write_file(&ctx, &json!({"path": "out.txt", "content": "hello"})).await;
        assert_eq!(result, "OK");
        let content = tokio::fs::read_to_string(dir.path().join("out.txt"))
            .await
            .unwrap();
        assert_eq!(content, "hello");
    }

    #[tokio::test]
    async fn write_file_creates_intermediate_dirs() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result =
            write_file(&ctx, &json!({"path": "a/b/c.txt", "content": "deep"})).await;
        assert_eq!(result, "OK");
        assert!(dir.path().join("a/b/c.txt").exists());
    }

    #[tokio::test]
    async fn list_dir_returns_entries() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("foo.rs"), "").await.unwrap();
        tokio::fs::create_dir(dir.path().join("subdir")).await.unwrap();
        let ctx = ctx(&dir);
        let result = list_dir(&ctx, &json!({})).await;
        assert!(result.contains("foo.rs"));
        assert!(result.contains("subdir/"));
    }

    #[tokio::test]
    async fn list_dir_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = list_dir(&ctx, &json!({"path": "../../etc"})).await;
        assert!(result.contains("Error"), "got: {result}");
    }

    #[tokio::test]
    async fn list_dir_empty_directory_returns_empty_message() {
        let dir = TempDir::new().unwrap();
        let inner = dir.path().join("empty");
        tokio::fs::create_dir(&inner).await.unwrap();
        let inner_ctx = ToolContext {
            proxy_url: String::new(),
            cwd: inner.to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
        };
        let result = list_dir(&inner_ctx, &json!({})).await;
        assert_eq!(result, "(empty directory)");
    }

    #[tokio::test]
    async fn list_dir_nonexistent_path_returns_error_or_empty() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        // A subdirectory that doesn't exist — resolve_path succeeds but the walker yields nothing
        let result = list_dir(&ctx, &json!({"path": "no_such_dir"})).await;
        // Either "(empty directory)" or an Error — both are acceptable signals
        assert!(
            result == "(empty directory)" || result.starts_with("Error"),
            "got: {result}"
        );
    }

    #[tokio::test]
    async fn list_dir_truncates_at_500_entries() {
        let dir = TempDir::new().unwrap();
        for i in 0..510 {
            tokio::fs::write(dir.path().join(format!("file_{i:04}.txt")), "")
                .await
                .unwrap();
        }
        let ctx = ctx(&dir);
        let result = list_dir(&ctx, &json!({})).await;
        assert!(
            result.contains("truncated at 500 entries"),
            "got: {result}"
        );
        let file_lines = result.lines().filter(|l| l.contains("file_")).count();
        assert_eq!(file_lines, 500, "expected exactly 500 file lines, got {file_lines}");
    }

    #[tokio::test]
    async fn glob_finds_matching_files() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("main.rs"), "").await.unwrap();
        tokio::fs::write(dir.path().join("lib.rs"), "").await.unwrap();
        tokio::fs::write(dir.path().join("readme.md"), "").await.unwrap();
        let ctx = ctx(&dir);
        let result = glob_files(&ctx, &json!({"pattern": "*.rs"})).await;
        assert!(result.contains("main.rs"));
        assert!(result.contains("lib.rs"));
        assert!(!result.contains("readme.md"));
    }

    #[tokio::test]
    async fn glob_path_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = glob_files(&ctx, &json!({"pattern": "*.rs", "path": "../../etc"})).await;
        assert!(result.contains("Error"), "got: {result}");
    }

    #[tokio::test]
    async fn glob_missing_pattern_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = glob_files(&ctx, &json!({})).await;
        assert!(result.contains("missing required parameter 'pattern'"), "got: {result}");
    }

    #[tokio::test]
    async fn glob_no_matches_returns_no_files_found() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("readme.md"), "").await.unwrap();
        let ctx = ctx(&dir);
        let result = glob_files(&ctx, &json!({"pattern": "*.rs"})).await;
        assert_eq!(result, "No files found");
    }

    #[tokio::test]
    async fn glob_invalid_pattern_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = glob_files(&ctx, &json!({"pattern": "[invalid"})).await;
        assert!(result.starts_with("Error"), "got: {result}");
    }

    #[tokio::test]
    async fn write_file_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result =
            write_file(&ctx, &json!({"path": "../../evil.txt", "content": "bad"})).await;
        assert!(result.contains("Error"), "got: {result}");
        assert!(!std::path::Path::new("/tmp/evil.txt").exists());
    }

    #[tokio::test]
    async fn notebook_edit_missing_path_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(&ctx, &json!({"cell_index": 0, "content": "x"})).await;
        assert!(result.contains("missing required parameter 'path'"), "got: {result}");
    }

    #[tokio::test]
    async fn notebook_edit_missing_cell_index_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(&ctx, &json!({"path": "nb.ipynb", "content": "x"})).await;
        assert!(result.contains("missing required parameter 'cell_index'"), "got: {result}");
    }

    #[tokio::test]
    async fn notebook_edit_missing_content_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(&ctx, &json!({"path": "nb.ipynb", "cell_index": 0})).await;
        assert!(result.contains("missing required parameter 'content'"), "got: {result}");
    }

    #[tokio::test]
    async fn notebook_edit_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(
            &ctx,
            &json!({"path": "../../evil.ipynb", "cell_index": 0, "content": "x"}),
        )
        .await;
        assert!(result.contains("Error"), "got: {result}");
    }

    #[tokio::test]
    async fn notebook_edit_out_of_range_index_returns_error() {
        let dir = TempDir::new().unwrap();
        let nb = serde_json::json!({
            "nbformat": 4,
            "cells": [
                {"cell_type": "code", "source": ["x"], "metadata": {}, "outputs": [], "execution_count": null}
            ]
        });
        tokio::fs::write(
            dir.path().join("nb.ipynb"),
            serde_json::to_string_pretty(&nb).unwrap(),
        )
        .await
        .unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(
            &ctx,
            &json!({"path": "nb.ipynb", "cell_index": 99, "content": "x"}),
        )
        .await;
        assert!(result.contains("Error"), "got: {result}");
        assert!(result.contains("out of range"), "got: {result}");
    }

    #[tokio::test]
    async fn notebook_edit_nonexistent_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(
            &ctx,
            &json!({"path": "no_such.ipynb", "cell_index": 0, "content": "x"}),
        )
        .await;
        assert!(result.starts_with("Error"), "got: {result}");
    }

    #[tokio::test]
    async fn notebook_edit_updates_cell() {
        let dir = TempDir::new().unwrap();
        let nb = serde_json::json!({
            "nbformat": 4,
            "cells": [
                {"cell_type": "code", "source": ["old content"], "metadata": {}, "outputs": [], "execution_count": null}
            ]
        });
        tokio::fs::write(
            dir.path().join("nb.ipynb"),
            serde_json::to_string_pretty(&nb).unwrap(),
        )
        .await
        .unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(
            &ctx,
            &json!({"path": "nb.ipynb", "cell_index": 0, "content": "new content"}),
        )
        .await;
        assert_eq!(result, "OK");
        let raw = tokio::fs::read_to_string(dir.path().join("nb.ipynb"))
            .await
            .unwrap();
        assert!(raw.contains("new content"));
    }

    #[tokio::test]
    async fn notebook_edit_changes_cell_type() {
        let dir = TempDir::new().unwrap();
        let nb = serde_json::json!({
            "nbformat": 4,
            "cells": [
                {"cell_type": "code", "source": ["x"], "metadata": {}, "outputs": [], "execution_count": null}
            ]
        });
        tokio::fs::write(
            dir.path().join("nb.ipynb"),
            serde_json::to_string_pretty(&nb).unwrap(),
        )
        .await
        .unwrap();
        let ctx = ctx(&dir);
        let result = notebook_edit(
            &ctx,
            &json!({"path": "nb.ipynb", "cell_index": 0, "content": "# title", "cell_type": "markdown"}),
        )
        .await;
        assert_eq!(result, "OK");
        let raw = tokio::fs::read_to_string(dir.path().join("nb.ipynb")).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["cells"][0]["cell_type"], "markdown");
    }
}
