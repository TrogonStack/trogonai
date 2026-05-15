use serde_json::Value;

use crate::{fs::resolve_path, ToolContext};

const MAX_MATCHES: usize = 100;
const MAX_OUTPUT_BYTES: usize = 32 * 1024;

pub(crate) async fn search_files(ctx: &ToolContext, input: &Value) -> String {
    let pattern = match input.get("pattern").and_then(|v| v.as_str()) {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => return "search_files: 'pattern' is required".to_string(),
    };
    let search_path = input
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let case_insensitive = input
        .get("case_insensitive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let base = match resolve_path(&ctx.cwd, search_path) {
        Ok(p) => p,
        Err(e) => return format!("search_files: {e}"),
    };

    let needle = if case_insensitive {
        pattern.to_lowercase()
    } else {
        pattern.clone()
    };

    // Walk files respecting .gitignore via the `ignore` crate.
    let walker = ignore::WalkBuilder::new(&base)
        .hidden(false)
        .git_ignore(true)
        .build();

    let mut output = String::new();
    let mut match_count = 0;
    let mut truncated = false;

    'walk: for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            continue;
        }

        let path = entry.path();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue, // skip binary / unreadable files
        };

        let rel = path
            .strip_prefix(&base)
            .unwrap_or(path)
            .to_string_lossy();

        for (line_no, line) in content.lines().enumerate() {
            let haystack = if case_insensitive {
                line.to_lowercase()
            } else {
                line.to_string()
            };

            if haystack.contains(&needle) {
                let entry_str = format!("{}:{}: {}\n", rel, line_no + 1, line.trim_end());
                if output.len() + entry_str.len() > MAX_OUTPUT_BYTES {
                    truncated = true;
                    break 'walk;
                }
                output.push_str(&entry_str);
                match_count += 1;
                if match_count >= MAX_MATCHES {
                    truncated = true;
                    break 'walk;
                }
            }
        }
    }

    if output.is_empty() {
        return format!("No matches found for '{pattern}'");
    }

    if truncated {
        output.push_str(&format!(
            "\n... ({match_count} matches shown, output truncated)"
        ));
    } else {
        output.push_str(&format!("\n{match_count} match(es)"));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    fn ctx(dir: &TempDir) -> ToolContext {
        ToolContext {
            proxy_url: String::new(),
            cwd: dir.path().to_string_lossy().into_owned(),
            http_client: reqwest::Client::new(),
        }
    }

    #[tokio::test]
    async fn finds_matching_lines() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.rs"), "fn hello() {}\nfn world() {}\n").unwrap();

        let result = search_files(&ctx(&dir), &json!({ "pattern": "hello" })).await;
        assert!(result.contains("a.rs:1:"), "must show file and line: {result}");
        assert!(result.contains("fn hello()"), "must show matching line: {result}");
    }

    #[tokio::test]
    async fn no_match_returns_message() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.rs"), "fn hello() {}\n").unwrap();

        let result = search_files(&ctx(&dir), &json!({ "pattern": "xyz_not_there" })).await;
        assert!(result.contains("No matches"), "{result}");
    }

    #[tokio::test]
    async fn case_insensitive_flag() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.rs"), "fn Hello() {}\n").unwrap();

        let sensitive = search_files(&ctx(&dir), &json!({ "pattern": "hello" })).await;
        assert!(sensitive.contains("No matches"), "case-sensitive must not match: {sensitive}");

        let insensitive = search_files(
            &ctx(&dir),
            &json!({ "pattern": "hello", "case_insensitive": true }),
        ).await;
        assert!(insensitive.contains("a.rs:1:"), "case-insensitive must match: {insensitive}");
    }

    #[tokio::test]
    async fn missing_pattern_returns_error() {
        let dir = TempDir::new().unwrap();
        let result = search_files(&ctx(&dir), &json!({})).await;
        assert!(result.contains("'pattern' is required"), "{result}");
    }

    #[tokio::test]
    async fn searches_subdirectory() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/b.rs"), "let needle = 42;\n").unwrap();
        fs::write(dir.path().join("other.rs"), "no match here\n").unwrap();

        let result = search_files(
            &ctx(&dir),
            &json!({ "pattern": "needle", "path": "sub" }),
        ).await;
        assert!(result.contains("b.rs:1:"), "{result}");
        assert!(!result.contains("other.rs"), "must only search in sub/: {result}");
    }
}
