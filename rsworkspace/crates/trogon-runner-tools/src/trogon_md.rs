use std::path::PathBuf;

/// Loads and concatenates all `TROGON.md` files relevant to `cwd`.
///
/// Concatenation order (most general → most specific):
/// 1. `~/.config/trogon/TROGON.md` — global user configuration
/// 2. All `TROGON.md` files found walking up from `cwd` to `/`,
///    ordered root-first so child directories can extend or override parents.
///
/// Returns `None` when no files are found.
pub async fn load_trogon_md(cwd: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();

    if let Some(content) = load_global().await {
        parts.push(content);
    }

    // Collect all candidates walking up from cwd, then reverse to get root-first order.
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut dir = PathBuf::from(cwd);
    loop {
        candidates.push(dir.join("TROGON.md"));
        if !dir.pop() {
            break;
        }
    }
    candidates.reverse();
    for path in candidates {
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            parts.push(content);
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

async fn load_global() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let path = PathBuf::from(home).join(".config/trogon/TROGON.md");
    tokio::fs::read_to_string(&path).await.ok()
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

    #[tokio::test]
    async fn returns_none_when_no_file_exists() {
        let dir = tmp_dir("none").await;
        let result = load_trogon_md(dir.to_str().unwrap()).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn loads_trogon_md_from_cwd() {
        let dir = tmp_dir("cwd").await;
        write(&dir.join("TROGON.md"), "cwd content").await;
        let result = load_trogon_md(dir.to_str().unwrap()).await.unwrap();
        assert!(result.contains("cwd content"), "got: {result}");
    }

    #[tokio::test]
    async fn parent_comes_before_child() {
        let parent = tmp_dir("parent_child_parent").await;
        let child = parent.join("child");
        tokio::fs::create_dir_all(&child).await.unwrap();

        write(&parent.join("TROGON.md"), "parent content").await;
        write(&child.join("TROGON.md"), "child content").await;

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
        assert!(root_pos < mid_pos && mid_pos < leaf_pos, "order must be root→mid→leaf, got: {result}");
    }

    // Serialize tests that override HOME to prevent parallel-test interference.
    static HOME_MUTEX: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    fn home_test_mutex() -> &'static std::sync::Mutex<()> {
        HOME_MUTEX.get_or_init(|| std::sync::Mutex::new(()))
    }

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
}
