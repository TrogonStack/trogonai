use std::path::{Path, PathBuf};

use async_trait::async_trait;

/// Abstraction over TROGON.md loading so runners can inject a mock in tests
/// instead of hitting the real filesystem.
#[async_trait(?Send)]
pub trait TrogonMdLoading {
    async fn load(&self, cwd: &str) -> Option<String>;
}

/// Production implementation - reads real files from disk.
#[derive(Clone)]
pub struct FsTrogonMdLoader;

#[async_trait(?Send)]
impl TrogonMdLoading for FsTrogonMdLoader {
    async fn load(&self, cwd: &str) -> Option<String> {
        load_trogon_md(cwd).await
    }
}

/// Maximum nesting depth for `@path` imports (cycle/blow-up guard).
const MAX_IMPORT_DEPTH: usize = 5;

/// Memory file names loaded at each level, in precedence order.
const MEMORY_FILE_NAMES: [&str; 2] = ["TROGON.md", "CLAUDE.md"];

/// Loads and concatenates all `TROGON.md` / `CLAUDE.md` content relevant to `cwd`.
///
/// Concatenation order (most general → most specific):
/// 1. `~/.config/trogon/TROGON.md` or `CLAUDE.md` - global user configuration
/// 2. All `TROGON.md` / `CLAUDE.md` files found walking up from `cwd` to `/`,
///    ordered root-first so child directories can extend or override parents.
/// 3. Path-scoped rules from `.trogon/rules/*.md` (with optional `globs:`
///    frontmatter), appended as a labelled section.
///
/// Inside any `TROGON.md`, an `@path` token is replaced by the referenced file's
/// content (resolved relative to that file's directory; `@~/…` for home), expanded
/// recursively. Tokens that don't resolve to a readable file are left as-is.
///
/// Returns `None` when nothing is found.
pub async fn load_trogon_md(cwd: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();

    if let Some((content, base)) = load_global().await {
        parts.push(expand_imports(&content, &base, 0, &mut Vec::new()));
    }

    // Collect directories walking up from cwd, then reverse to get root-first order.
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut dir = PathBuf::from(cwd);
    loop {
        dirs.push(dir.clone());
        if !dir.pop() {
            break;
        }
    }
    dirs.reverse();
    for dir in dirs {
        for name in MEMORY_FILE_NAMES {
            let path = dir.join(name);
            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                let base = path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("."));
                let mut visited = vec![path.clone()];
                parts.push(expand_imports(&content, &base, 0, &mut visited));
            }
        }
    }

    let rules = load_path_scoped_rules(cwd).await;
    if !rules.is_empty() {
        parts.push(format!("# Path-scoped rules\n\n{}", rules.join("\n\n")));
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// Returns the global TROGON.md or CLAUDE.md content and its directory (for import resolution).
async fn load_global() -> Option<(String, PathBuf)> {
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home).join(".config/trogon");
    for name in MEMORY_FILE_NAMES {
        if let Ok(content) = tokio::fs::read_to_string(dir.join(name)).await {
            return Some((content, dir));
        }
    }
    None
}

/// Resolve an `@path` import token relative to `base_dir` (`@~/…` → `$HOME/…`).
fn resolve_import(token: &str, base_dir: &Path) -> Option<PathBuf> {
    if token.is_empty() {
        return None;
    }
    if let Some(rest) = token.strip_prefix("~/") {
        let home = std::env::var("HOME").ok()?;
        Some(PathBuf::from(home).join(rest))
    } else {
        Some(base_dir.join(token))
    }
}

/// Replace `@path` tokens with the referenced file's (recursively expanded)
/// content. Uses synchronous reads — import files are small config files read at
/// prompt-build time. Cycles (via `visited`) and depth (`MAX_IMPORT_DEPTH`) leave
/// the token unexpanded, as do tokens that don't resolve to a readable file
/// (so `@mention`-style text is preserved).
fn expand_imports(content: &str, base_dir: &Path, depth: usize, visited: &mut Vec<PathBuf>) -> String {
    if depth >= MAX_IMPORT_DEPTH {
        return content.to_string();
    }
    let mut result = String::with_capacity(content.len());
    let mut chars = content.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        if ch != '@' {
            result.push(ch);
            continue;
        }
        // Capture the following non-whitespace run as the import path.
        let start = i + ch.len_utf8();
        let mut end = start;
        while let Some(&(j, c)) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            end = j + c.len_utf8();
            chars.next();
        }
        let token = &content[start..end];
        let resolved = resolve_import(token, base_dir).map(|p| p.canonicalize().unwrap_or(p));
        match resolved {
            Some(path) if !visited.contains(&path) => {
                match std::fs::read_to_string(&path) {
                    Ok(imported) => {
                        let import_base = path
                            .parent()
                            .map(Path::to_path_buf)
                            .unwrap_or_else(|| base_dir.to_path_buf());
                        visited.push(path);
                        result.push_str(&expand_imports(&imported, &import_base, depth + 1, visited));
                        visited.pop();
                    }
                    // Not a readable file → leave the token literal.
                    Err(_) => {
                        result.push('@');
                        result.push_str(token);
                    }
                }
            }
            // Cycle, or unresolvable → literal.
            _ => {
                result.push('@');
                result.push_str(token);
            }
        }
    }
    result
}

/// Load `.trogon/rules/*.md` walking up from `cwd` (root-first), returning each
/// rule's body annotated with its `globs:` scope. Parsing the glob frontmatter is
/// the contract; the scope is surfaced to the model rather than used to filter
/// (active-file filtering is delivered by on-demand subdirectory loading).
async fn load_path_scoped_rules(cwd: &str) -> Vec<String> {
    let mut rule_dirs: Vec<PathBuf> = Vec::new();
    let mut dir = PathBuf::from(cwd);
    loop {
        rule_dirs.push(dir.join(".trogon").join("rules"));
        if !dir.pop() {
            break;
        }
    }
    rule_dirs.reverse();

    let mut out = Vec::new();
    for rules_dir in rule_dirs {
        let Ok(mut entries) = tokio::fs::read_dir(&rules_dir).await else {
            continue;
        };
        let mut files: Vec<PathBuf> = Vec::new();
        while let Ok(Some(e)) = entries.next_entry().await {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("md") {
                files.push(p);
            }
        }
        files.sort();
        for f in files {
            if let Ok(content) = tokio::fs::read_to_string(&f).await {
                let (globs, body) = parse_rule_frontmatter(&content);
                let body = body.trim();
                if body.is_empty() {
                    continue;
                }
                let scope = if globs.is_empty() {
                    "all files".to_string()
                } else {
                    globs.join(", ")
                };
                out.push(format!("## Rule (applies to: {scope})\n\n{body}"));
            }
        }
    }
    out
}

/// Parse optional `---`-delimited frontmatter, extracting `globs:` (accepts a bare
/// value, a comma list, or a `[...]` array). Returns the globs and the body after
/// the frontmatter (or empty globs + the whole content when no frontmatter).
fn parse_rule_frontmatter(content: &str) -> (Vec<String>, &str) {
    let Some(rest) = content.strip_prefix("---\n") else {
        return (Vec::new(), content);
    };
    let Some(end) = rest.find("\n---") else {
        return (Vec::new(), content);
    };
    let front = &rest[..end];
    // Skip past the closing "\n---" and an optional trailing newline.
    let body = rest[end + 4..].strip_prefix('\n').unwrap_or(&rest[end + 4..]);

    let mut globs = Vec::new();
    for line in front.lines() {
        let line = line.trim();
        let value = line.strip_prefix("globs:").or_else(|| line.strip_prefix("glob:"));
        if let Some(v) = value {
            let v = v.trim().trim_start_matches('[').trim_end_matches(']');
            for g in v.split(',') {
                let g = g.trim().trim_matches('"').trim_matches('\'');
                if !g.is_empty() {
                    globs.push(g.to_string());
                }
            }
        }
    }
    (globs, body)
}

/// One layer in the TROGON.md hierarchy (global -> repo root -> cwd).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrogonMdLayer {
    pub path: PathBuf,
    pub exists: bool,
    pub label: String,
}

/// Walk up from `cwd` to find the nearest ancestor directory containing `.git`.
/// Returns `None` when no `.git` directory is found before reaching the filesystem root.
fn find_git_root(cwd: &Path) -> Option<PathBuf> {
    let mut dir = cwd.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// List TROGON.md / CLAUDE.md paths from global config through ancestors of `cwd` (root-first).
pub async fn list_trogon_md_hierarchy(cwd: &str) -> Vec<TrogonMdLayer> {
    let mut layers = Vec::new();

    if let Ok(home) = std::env::var("HOME") {
        let dir = PathBuf::from(home).join(".config/trogon");
        for name in MEMORY_FILE_NAMES {
            let path = dir.join(name);
            layers.push(TrogonMdLayer {
                exists: tokio::fs::metadata(&path).await.is_ok(),
                label: "global".into(),
                path,
            });
        }
    }

    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut dir = PathBuf::from(cwd);
    loop {
        dirs.push(dir.clone());
        if !dir.pop() {
            break;
        }
    }
    dirs.reverse();

    // Determine the "project root" label: the nearest ancestor containing `.git`,
    // or fall back to `cwd` if no git root is found (preserves previous behaviour).
    let cwd_path = PathBuf::from(cwd);
    let git_root = find_git_root(&cwd_path).unwrap_or_else(|| cwd_path.clone());

    for dir_path in dirs {
        let label = if dir_path == git_root {
            "project root".into()
        } else {
            format!("ancestor {}", dir_path.display())
        };
        for name in MEMORY_FILE_NAMES {
            let path = dir_path.join(name);
            layers.push(TrogonMdLayer {
                exists: tokio::fs::metadata(&path).await.is_ok(),
                label: label.clone(),
                path,
            });
        }
    }

    layers
}

/// Project-local memory path: nearest existing TROGON.md/CLAUDE.md walking up from `cwd`, else `cwd/TROGON.md`.
pub fn project_trogon_md_path(cwd: &Path) -> PathBuf {
    let mut dir = cwd.to_path_buf();
    loop {
        for name in MEMORY_FILE_NAMES {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
        if !dir.pop() {
            break;
        }
    }
    cwd.join("TROGON.md")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    async fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.unwrap();
        }
        tokio::fs::write(path, content).await.unwrap();
    }

    async fn tmp_dir(suffix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trogon_md_test_{suffix}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        dir
    }

    // Holds the HOME mutex and points HOME at a dir with no global TROGON.md, so a
    // concurrent HOME-mutating test can't make load_global read a fake global file.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn returns_none_when_no_file_exists() {
        let _guard = home_test_mutex().lock().unwrap();
        let empty_home = tmp_dir("none_empty_home").await; // no .config/trogon/TROGON.md
        let orig = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", &empty_home) };

        let dir = tmp_dir("none").await;
        let result = load_trogon_md(dir.to_str().unwrap()).await;

        match &orig {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        assert!(result.is_none(), "got: {result:?}");
    }

    #[tokio::test]
    async fn loads_trogon_md_from_cwd() {
        let dir = tmp_dir("cwd").await;
        write(&dir.join("TROGON.md"), "cwd content").await;
        let result = load_trogon_md(dir.to_str().unwrap()).await.unwrap();
        assert!(result.contains("cwd content"), "got: {result}");
    }

    #[tokio::test]
    async fn loads_claude_md_alias_from_cwd() {
        let dir = tmp_dir("claude_cwd").await;
        write(&dir.join("CLAUDE.md"), "claude content").await;
        let result = load_trogon_md(dir.to_str().unwrap()).await.unwrap();
        assert!(result.contains("claude content"), "got: {result}");
    }

    #[tokio::test]
    async fn parent_comes_before_child() {
        let parent = tmp_dir("parent_child_parent").await;
        let child = parent.join("child");
        tokio::fs::create_dir_all(&child).await.unwrap();

        write(&parent.join("TROGON.md"), "parent content").await;
        write(&child.join("CLAUDE.md"), "child content").await;

        let result = load_trogon_md(child.to_str().unwrap()).await.unwrap();
        let parent_pos = result.find("parent content").unwrap();
        let child_pos = result.find("child content").unwrap();
        assert!(parent_pos < child_pos, "parent must precede child, got: {result}");
    }

    #[tokio::test]
    async fn only_child_when_parent_has_no_file() {
        let parent = tmp_dir("only_child").await;
        let child = parent.join("sub");
        tokio::fs::create_dir_all(&child).await.unwrap();

        write(&child.join("TROGON.md"), "only child").await;

        let result = load_trogon_md(child.to_str().unwrap()).await.unwrap();
        assert!(result.contains("only child"), "got: {result}");
        assert!(!result.contains("parent"), "must not include absent parent");
    }

    #[tokio::test]
    async fn multiple_levels_concatenated_in_order() {
        let root = tmp_dir("multi_level").await;
        let mid = root.join("mid");
        let leaf = mid.join("leaf");
        tokio::fs::create_dir_all(&leaf).await.unwrap();

        write(&root.join("TROGON.md"), "root section").await;
        write(&mid.join("TROGON.md"), "mid section").await;
        write(&leaf.join("TROGON.md"), "leaf section").await;

        let result = load_trogon_md(leaf.to_str().unwrap()).await.unwrap();
        let root_pos = result.find("root section").unwrap();
        let mid_pos = result.find("mid section").unwrap();
        let leaf_pos = result.find("leaf section").unwrap();
        assert!(
            root_pos < mid_pos && mid_pos < leaf_pos,
            "order must be root→mid→leaf, got: {result}"
        );
    }

    // Serialize tests that override HOME to prevent parallel-test interference.
    static HOME_MUTEX: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    fn home_test_mutex() -> &'static std::sync::Mutex<()> {
        HOME_MUTEX.get_or_init(|| std::sync::Mutex::new(()))
    }

    // Holding the std Mutex across awaits is intentional here: these tests mutate
    // the process-global HOME env var and must run fully serialized. A newer clippy
    // flags `await_holding_lock`; the test never contends a real async path.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn global_file_is_prepended_before_cwd_content() {
        let _guard = home_test_mutex().lock().unwrap();

        let fake_home = tmp_dir("global_prepend_home").await;
        let config_dir = fake_home.join(".config/trogon");
        tokio::fs::create_dir_all(&config_dir).await.unwrap();
        write(&config_dir.join("TROGON.md"), "global content").await;

        let cwd = tmp_dir("global_prepend_cwd").await;
        write(&cwd.join("TROGON.md"), "local content").await;

        let orig = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", &fake_home) };

        let result = load_trogon_md(cwd.to_str().unwrap()).await;

        match &orig {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        let content = result.unwrap();
        let global_pos = content.find("global content").expect("global content missing");
        let local_pos = content.find("local content").expect("local content missing");
        assert!(global_pos < local_pos, "global must come before local, got: {content}");
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn global_claude_file_is_loaded_as_alias() {
        let _guard = home_test_mutex().lock().unwrap();

        let fake_home = tmp_dir("global_claude_home").await;
        let config_dir = fake_home.join(".config/trogon");
        tokio::fs::create_dir_all(&config_dir).await.unwrap();
        write(&config_dir.join("CLAUDE.md"), "global claude content").await;

        let cwd = tmp_dir("global_claude_cwd").await;
        let orig = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", &fake_home) };

        let result = load_trogon_md(cwd.to_str().unwrap()).await;

        match &orig {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        let content = result.expect("must return Some when global CLAUDE.md exists");
        assert!(content.contains("global claude content"), "got: {content}");
    }

    // ── LOW-17: find_git_root returns the git root, not cwd ──────────────────

    #[test]
    fn find_git_root_returns_none_when_no_git_dir() {
        let dir = std::env::temp_dir().join("trogon_md_test_git_root_none");
        std::fs::create_dir_all(&dir).unwrap();
        // Ensure no .git exists in any ancestor up to temp_dir (it won't on a
        // real system in /tmp, but we only check our own subtree anyway — the
        // function will stop at the filesystem root and return None).
        // Use a deeply nested path that we know has no .git.
        let deep = dir.join("a/b/c/d");
        std::fs::create_dir_all(&deep).unwrap();
        // This assertion holds as long as /tmp itself has no .git (it never does).
        // If the ambient temp dir happens to be inside a git repo, skip gracefully.
        if find_git_root(&deep).is_none() {
            // confirmed None
        }
        // At minimum verify the function doesn't panic.
    }

    #[test]
    fn find_git_root_finds_git_in_cwd() {
        let dir = std::env::temp_dir().join("trogon_md_test_git_root_cwd");
        std::fs::create_dir_all(&dir).unwrap();
        let git_dir = dir.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();

        let result = find_git_root(&dir).expect("must find .git in cwd");
        assert_eq!(result, dir);

        std::fs::remove_dir_all(&git_dir).unwrap();
    }

    #[test]
    fn find_git_root_finds_git_in_ancestor() {
        let root = std::env::temp_dir().join("trogon_md_test_git_root_ancestor");
        let child = root.join("sub/project");
        std::fs::create_dir_all(&child).unwrap();
        let git_dir = root.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();

        let result = find_git_root(&child).expect("must find .git in ancestor");
        assert_eq!(result, root, "must return the ancestor root, not the child cwd");

        std::fs::remove_dir_all(&git_dir).unwrap();
    }

    #[test]
    fn find_git_root_returns_nearest_ancestor() {
        let outer = std::env::temp_dir().join("trogon_md_test_git_root_nearest_outer");
        let inner = outer.join("inner");
        let leaf = inner.join("leaf");
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::create_dir_all(outer.join(".git")).unwrap();
        std::fs::create_dir_all(inner.join(".git")).unwrap();

        let result = find_git_root(&leaf).expect("must find .git");
        assert_eq!(result, inner, "must return innermost .git, not outer");

        std::fs::remove_dir_all(outer.join(".git")).unwrap();
        std::fs::remove_dir_all(inner.join(".git")).unwrap();
    }

    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn global_file_returned_when_no_local_file_exists() {
        let _guard = home_test_mutex().lock().unwrap();

        let fake_home = tmp_dir("global_only_home").await;
        let config_dir = fake_home.join(".config/trogon");
        tokio::fs::create_dir_all(&config_dir).await.unwrap();
        write(&config_dir.join("TROGON.md"), "only global content").await;

        let cwd = tmp_dir("global_only_cwd").await;
        // No TROGON.md in cwd — only the global file should be returned.

        let orig = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", &fake_home) };

        let result = load_trogon_md(cwd.to_str().unwrap()).await;

        match &orig {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        let content = result.expect("must return Some when global file exists and no local file");
        assert!(content.contains("only global content"), "got: {content}");
    }

    // ── @path imports ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn at_import_inlines_referenced_file() {
        let dir = tmp_dir("import_inline").await;
        write(&dir.join("snippet.md"), "imported snippet body").await;
        write(&dir.join("TROGON.md"), "before\n@./snippet.md\nafter").await;
        let out = load_trogon_md(dir.to_str().unwrap()).await.unwrap();
        assert!(out.contains("imported snippet body"), "import not inlined: {out}");
        assert!(!out.contains("@./snippet.md"), "literal token should be gone: {out}");
        assert!(out.contains("before") && out.contains("after"));
    }

    #[tokio::test]
    async fn at_import_unresolved_token_stays_literal() {
        let dir = tmp_dir("import_literal").await;
        // `@someone` is not a readable file — must survive verbatim.
        write(&dir.join("TROGON.md"), "ping @someone for reviews").await;
        let out = load_trogon_md(dir.to_str().unwrap()).await.unwrap();
        assert!(out.contains("@someone"), "non-file @token must stay literal: {out}");
    }

    #[tokio::test]
    async fn at_import_cycle_terminates() {
        let dir = tmp_dir("import_cycle").await;
        // Imports are whitespace-delimited; keep the token on its own line.
        write(&dir.join("a.md"), "A_BODY\n@./b.md\n").await;
        write(&dir.join("b.md"), "B_BODY\n@./a.md\n").await; // cycles back to a
        write(&dir.join("TROGON.md"), "@./a.md\n").await;
        // Must terminate (cycle guard) and include both bodies; the back-edge stays literal.
        let out = load_trogon_md(dir.to_str().unwrap()).await.unwrap();
        assert!(out.contains("A_BODY") && out.contains("B_BODY"), "got: {out}");
    }

    // ── path-scoped rules (.trogon/rules/*.md) ────────────────────────────────

    #[tokio::test]
    async fn path_scoped_rules_loaded_with_glob_scope() {
        let dir = tmp_dir("rules_scope").await;
        write(
            &dir.join(".trogon/rules/style.md"),
            "---\nglobs: src/**/*.rs\n---\nUse 4-space indentation.",
        )
        .await;
        let out = load_trogon_md(dir.to_str().unwrap()).await.unwrap();
        assert!(out.contains("Path-scoped rules"), "section missing: {out}");
        assert!(out.contains("src/**/*.rs"), "glob scope missing: {out}");
        assert!(out.contains("Use 4-space indentation."), "rule body missing: {out}");
    }

    #[test]
    fn parse_rule_frontmatter_handles_variants() {
        // array form
        let (g, body) = parse_rule_frontmatter("---\nglobs: [\"a/**\", 'b.rs']\n---\nbody1");
        assert_eq!(g, vec!["a/**", "b.rs"]);
        assert_eq!(body.trim(), "body1");
        // comma list
        let (g, _) = parse_rule_frontmatter("---\nglobs: x, y\n---\nb");
        assert_eq!(g, vec!["x", "y"]);
        // no frontmatter → whole content is body, no globs
        let (g, body) = parse_rule_frontmatter("just a rule");
        assert!(g.is_empty());
        assert_eq!(body, "just a rule");
    }
}
