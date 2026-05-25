use serde_json::Value;

use crate::ToolContext;
use crate::fs::{resolve_path, unique_tmp_path};

pub async fn str_replace(ctx: &ToolContext, input: &Value) -> String {
    let path = match input.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => return "Error: missing required parameter 'path'".to_string(),
    };
    let old_str = match input.get("old_str").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return "Error: missing required parameter 'old_str'".to_string(),
    };
    let new_str = match input.get("new_str").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return "Error: missing required parameter 'new_str'".to_string(),
    };

    let full_path = match resolve_path(&ctx.cwd, path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {e}"),
    };

    let content = match tokio::fs::read_to_string(&full_path).await {
        Ok(c) => c,
        Err(e) => return format!("Error reading {path}: {e}"),
    };

    let count = content.matches(old_str).count();
    match count {
        0 => return "Error: 'old_str' not found in file (0 occurrences)".to_string(),
        1 => {}
        n => {
            return format!(
                "Error: 'old_str' is not unique — found {n} occurrences. \
                 Add more surrounding context to make it unique."
            )
        }
    }

    let updated = content.replacen(old_str, new_str, 1);

    let tmp = unique_tmp_path(&full_path);
    if let Err(e) = tokio::fs::write(&tmp, &updated).await {
        return format!("Error writing file: {e}");
    }
    if let Err(e) = tokio::fs::rename(&tmp, &full_path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return format!("Error saving file: {e}");
    }

    diff_context(
        &content.lines().collect::<Vec<_>>(),
        &updated.lines().collect::<Vec<_>>(),
        3,
    )
}

fn diff_context(old: &[&str], new: &[&str], context: usize) -> String {
    // MED-14: a positional `old[i] vs new[i]` comparison misaligns every line
    // after an insertion or deletion, so unchanged lines show up as edits.
    // str_replace always rewrites one contiguous block, so the real diff is a
    // common prefix, a changed middle, and a common suffix. Find those instead.
    let mut prefix = 0;
    while prefix < old.len() && prefix < new.len() && old[prefix] == new[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old.len() - prefix
        && suffix < new.len() - prefix
        && old[old.len() - 1 - suffix] == new[new.len() - 1 - suffix]
    {
        suffix += 1;
    }

    if prefix == old.len() && prefix == new.len() {
        return "No changes".to_string();
    }

    let old_changed_end = old.len() - suffix; // exclusive
    let new_changed_end = new.len() - suffix; // exclusive

    let mut out = Vec::new();
    // Leading context (identical in old and new).
    for line in old.iter().take(prefix).skip(prefix.saturating_sub(context)) {
        out.push(format!("  {line}"));
    }
    // Removed then added lines for the changed block.
    for line in old.iter().take(old_changed_end).skip(prefix) {
        out.push(format!("- {line}"));
    }
    for line in new.iter().take(new_changed_end).skip(prefix) {
        out.push(format!("+ {line}"));
    }
    // Trailing context (identical in old and new).
    let trail_end = (old_changed_end + context).min(old.len());
    for line in old.iter().take(trail_end).skip(old_changed_end) {
        out.push(format!("  {line}"));
    }

    out.join("\n")
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

    #[tokio::test]
    async fn str_replace_missing_path_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = str_replace(&ctx, &json!({"old_str": "x", "new_str": "y"})).await;
        assert!(result.contains("missing required parameter 'path'"), "got: {result}");
    }

    #[tokio::test]
    async fn str_replace_missing_old_str_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = str_replace(&ctx, &json!({"path": "f.rs", "new_str": "y"})).await;
        assert!(result.contains("missing required parameter 'old_str'"), "got: {result}");
    }

    #[tokio::test]
    async fn str_replace_missing_new_str_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = str_replace(&ctx, &json!({"path": "f.rs", "old_str": "x"})).await;
        assert!(result.contains("missing required parameter 'new_str'"), "got: {result}");
    }

    #[tokio::test]
    async fn str_replace_replaces_unique_occurrence() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("f.rs"), "fn foo() {}\nfn bar() {}\n")
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = str_replace(
            &ctx,
            &json!({"path": "f.rs", "old_str": "fn foo() {}", "new_str": "fn baz() {}"}),
        )
        .await;
        assert!(!result.starts_with("Error"), "got: {result}");
        let content = tokio::fs::read_to_string(dir.path().join("f.rs"))
            .await
            .unwrap();
        assert!(content.contains("fn baz() {}"));
        assert!(!content.contains("fn foo() {}"));
    }

    #[tokio::test]
    async fn str_replace_rejects_zero_occurrences() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("f.rs"), "fn foo() {}\n")
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = str_replace(
            &ctx,
            &json!({"path": "f.rs", "old_str": "fn missing() {}", "new_str": "x"}),
        )
        .await;
        assert!(result.contains("0 occurrences"), "got: {result}");
    }

    #[tokio::test]
    async fn str_replace_rejects_duplicate_occurrences() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("f.rs"), "foo\nfoo\n")
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = str_replace(
            &ctx,
            &json!({"path": "f.rs", "old_str": "foo", "new_str": "bar"}),
        )
        .await;
        assert!(result.contains("2 occurrences"), "got: {result}");
    }

    #[tokio::test]
    async fn str_replace_on_nonexistent_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = str_replace(
            &ctx,
            &json!({"path": "missing.rs", "old_str": "x", "new_str": "y"}),
        )
        .await;
        assert!(result.starts_with("Error"), "got: {result}");
    }

    #[tokio::test]
    async fn str_replace_traversal_rejected() {
        let dir = TempDir::new().unwrap();
        let ctx = ctx(&dir);
        let result = str_replace(
            &ctx,
            &json!({"path": "../../etc/passwd", "old_str": "root", "new_str": "x"}),
        )
        .await;
        assert!(result.contains("Error"), "got: {result}");
    }

    #[tokio::test]
    async fn str_replace_diff_shows_changed_lines() {
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("f.txt"), "aaa\nbbb\nccc\n")
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = str_replace(
            &ctx,
            &json!({"path": "f.txt", "old_str": "bbb", "new_str": "BBB"}),
        )
        .await;
        assert!(result.contains("- bbb"), "expected removal marker, got: {result}");
        assert!(result.contains("+ BBB"), "expected addition marker, got: {result}");
    }

    #[tokio::test]
    async fn str_replace_diff_handles_line_count_change() {
        // MED-14: replacing one line with two must not mark the unchanged
        // trailing lines as modified.
        let dir = TempDir::new().unwrap();
        tokio::fs::write(dir.path().join("f.txt"), "a\nb\nc\nd\n")
            .await
            .unwrap();
        let ctx = ctx(&dir);
        let result = str_replace(
            &ctx,
            &json!({"path": "f.txt", "old_str": "b", "new_str": "b1\nb2"}),
        )
        .await;
        assert!(result.contains("- b"), "got: {result}");
        assert!(result.contains("+ b1") && result.contains("+ b2"), "got: {result}");
        assert!(!result.contains("- c"), "c must stay unchanged, got: {result}");
        assert!(!result.contains("- d"), "d must stay unchanged, got: {result}");
    }
}
