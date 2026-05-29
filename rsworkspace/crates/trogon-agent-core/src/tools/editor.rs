use serde_json::Value;

use crate::tools::ToolContext;
use crate::tools::fs::resolve_path;

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

    let tmp = full_path.with_extension(format!(
        "{}.tmp",
        full_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("tmp")
    ));
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
    let max_len = old.len().max(new.len());
    let changed: Vec<usize> = (0..max_len)
        .filter(|&i| old.get(i).copied().unwrap_or("") != new.get(i).copied().unwrap_or(""))
        .collect();

    if changed.is_empty() {
        return "No changes".to_string();
    }

    let first = changed[0].saturating_sub(context);
    let last = (changed[changed.len() - 1] + context + 1).min(max_len);

    let mut out = Vec::new();
    for i in first..last {
        let o = old.get(i).copied().unwrap_or("");
        let n = new.get(i).copied().unwrap_or("");
        if i >= old.len() {
            out.push(format!("+ {n}"));
        } else if i >= new.len() {
            out.push(format!("- {o}"));
        } else if o != n {
            out.push(format!("- {o}"));
            out.push(format!("+ {n}"));
        } else {
            out.push(format!("  {o}"));
        }
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
}
