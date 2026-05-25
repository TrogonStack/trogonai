//! Load `rsworkspace/.env.local` so `trogon doctor` and the REPL see API keys
//! without manual `source .env.local`.

use std::path::{Path, PathBuf};

/// Load the first `.env.local` found (does not override existing env vars).
pub fn load_env_local() {
    if let Some(path) = find_env_local() {
        let _ = load_file(&path);
    }
}

/// Resolve `.env.local` — cwd ancestors, then next to the trogon binary (dev layout).
pub fn find_env_local() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("TROGON_ENV_FILE") {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Some(p);
        }
    }

    if let Ok(cwd) = std::env::current_dir()
        && let Some(p) = find_in_ancestors(&cwd, ".env.local")
    {
        return Some(p);
    }

    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for rel in ["../../.env.local", "../../../.env.local"] {
            let candidate = dir.join(rel);
            if candidate.is_file() {
                return candidate.canonicalize().ok();
            }
        }
    }

    None
}

fn find_in_ancestors(start: &Path, name: &str) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn load_file(path: &Path) -> std::io::Result<()> {
    let content = std::fs::read_to_string(path)?;
    for line in content.lines() {
        apply_line(line);
    }
    Ok(())
}

fn apply_line(line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return;
    }
    let Some((key, value)) = trimmed.split_once('=') else {
        return;
    };
    let key = key.trim();
    if key.is_empty() {
        return;
    }
    if std::env::var(key)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
    {
        return;
    }
    let value = strip_quotes(value.trim());
    unsafe { std::env::set_var(key, value) };
}

fn strip_quotes(value: &str) -> &str {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return &value[1..value.len() - 1];
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn apply_line_sets_unset_var() {
        let _lock = ENV_LOCK.lock().unwrap();
        let key = "TROGON_ENV_LOCAL_TEST_KEY";
        unsafe { std::env::remove_var(key) };
        apply_line(&format!("{key}=hello"));
        assert_eq!(std::env::var(key).unwrap(), "hello");
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn apply_line_does_not_override_nonempty_existing() {
        let _lock = ENV_LOCK.lock().unwrap();
        let key = "TROGON_ENV_LOCAL_TEST_OVERRIDE";
        unsafe { std::env::set_var(key, "from_shell") };
        apply_line(&format!("{key}=from_file"));
        assert_eq!(std::env::var(key).unwrap(), "from_shell");
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn apply_line_replaces_empty_existing() {
        let _lock = ENV_LOCK.lock().unwrap();
        let key = "TROGON_ENV_LOCAL_TEST_EMPTY";
        unsafe { std::env::set_var(key, "   ") };
        apply_line(&format!("{key}=from_file"));
        assert_eq!(std::env::var(key).unwrap(), "from_file");
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn strip_quotes_handles_double_and_single() {
        assert_eq!(strip_quotes("\"abc\""), "abc");
        assert_eq!(strip_quotes("'xyz'"), "xyz");
        assert_eq!(strip_quotes("plain"), "plain");
    }
}
