//! REPL session transcript helpers for `/export`, `/history`, and `/import`.
//!
//! Transcript payloads come from the runner's `session/export` ext method (same
//! source as cross-runner switching and `/checkpoint`).

use crate::fs::Fs;
use crate::session_store::expand_tilde;
use std::path::{Path, PathBuf};
use trogon_runner_tools::portable_session::{
    ParsedExport, parse_export_json, v2_message_to_text,
};

pub const EXPORTS_DIR: &str = "~/.local/share/trogon/exports";

const PREVIEW_MAX: usize = 80;

/// Default export path: `~/.local/share/trogon/exports/<session_id>.json`.
pub fn default_export_path(session_id: &str) -> PathBuf {
    expand_tilde(EXPORTS_DIR).join(format!("{session_id}.json"))
}

/// Resolve `/export [path]` — empty arg uses [`default_export_path`].
pub fn resolve_export_path(arg: &str, session_id: &str, cwd: &Path) -> PathBuf {
    let arg = arg.trim();
    if arg.is_empty() {
        return default_export_path(session_id);
    }
    resolve_user_path(arg, cwd)
}

/// Resolve `/import <path>` — errors when the path argument is missing.
pub fn resolve_import_path(arg: &str, cwd: &Path) -> Result<PathBuf, String> {
    let arg = arg.trim();
    if arg.is_empty() {
        return Err("usage: /import <path>".into());
    }
    Ok(resolve_user_path(arg, cwd))
}

fn resolve_user_path(arg: &str, cwd: &Path) -> PathBuf {
    if arg.starts_with("~/") {
        expand_tilde(arg)
    } else if Path::new(arg).is_absolute() {
        PathBuf::from(arg)
    } else {
        cwd.join(arg)
    }
}

/// Write a `session/export` JSON payload to disk.
pub fn write_export_file<F: Fs>(path: &Path, json: &str, fs: &F) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs.create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    fs.write_atomic(path, json.as_bytes())
        .map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// Concise numbered list of messages (role + truncated preview).
pub fn format_history_list(export_json: &str) -> Result<String, String> {
    let parsed = parse_export_json(export_json.trim())
        .map_err(|e| format!("invalid session export: {e}"))?;
    let lines = history_lines(&parsed);
    if lines.is_empty() {
        Ok("(no messages)".into())
    } else {
        Ok(lines.join("\n"))
    }
}

/// One-line summary after validating an export file.
pub fn format_import_summary(export_json: &str) -> Result<String, String> {
    let parsed = parse_export_json(export_json.trim())
        .map_err(|e| format!("invalid transcript: {e}"))?;
    let (total, users, assistants) = count_messages(&parsed);
    let version = match &parsed {
        ParsedExport::V1(_) => "v1",
        ParsedExport::V2(_) => "v2",
    };
    Ok(format!(
        "{total} messages ({users} user, {assistants} assistant) · export {version}"
    ))
}

fn history_lines(parsed: &ParsedExport) -> Vec<String> {
    match parsed {
        ParsedExport::V1(msgs) => msgs
            .iter()
            .enumerate()
            .map(|(i, m)| {
                format!(
                    "{:>3}  {:<9}  {}",
                    i + 1,
                    m.role,
                    truncate_preview(&m.text, PREVIEW_MAX)
                )
            })
            .collect(),
        ParsedExport::V2(exp) => exp
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let text = v2_message_to_text(m).text;
                format!(
                    "{:>3}  {:<9}  {}",
                    i + 1,
                    m.role,
                    truncate_preview(&text, PREVIEW_MAX)
                )
            })
            .collect(),
    }
}

fn count_messages(parsed: &ParsedExport) -> (usize, usize, usize) {
    let roles: Vec<&str> = match parsed {
        ParsedExport::V1(msgs) => msgs.iter().map(|m| m.role.as_str()).collect(),
        ParsedExport::V2(exp) => exp.messages.iter().map(|m| m.role.as_str()).collect(),
    };
    let total = roles.len();
    let users = roles.iter().filter(|r| **r == "user").count();
    let assistants = roles.iter().filter(|r| **r == "assistant").count();
    (total, users, assistants)
}

fn truncate_preview(s: &str, max: usize) -> String {
    let collapsed: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.len() <= max {
        collapsed
    } else {
        format!("{}…", &collapsed[..max.saturating_sub(1)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::mock::MockFs;

    const V1_EXPORT: &str = r#"[
        {"role":"user","text":"hello there"},
        {"role":"assistant","text":"hi back"}
    ]"#;

    const V2_EXPORT: &str = r#"{"version":2,"messages":[
        {"version":2,"role":"user","blocks":[{"type":"text","text":"question"}]},
        {"version":2,"role":"assistant","blocks":[{"type":"text","text":"answer with many words that should be truncated when displayed in the history list view"}]}
    ]}"#;

    #[test]
    fn default_export_path_uses_exports_dir() {
        let path = default_export_path("sess-abc");
        assert!(
            path.to_string_lossy().contains("trogon/exports/sess-abc.json"),
            "got {}",
            path.display()
        );
    }

    #[test]
    fn resolve_export_path_empty_uses_default() {
        let path = resolve_export_path("", "s1", Path::new("/tmp"));
        assert_eq!(path, default_export_path("s1"));
    }

    #[test]
    fn resolve_export_path_relative_uses_cwd() {
        let path = resolve_export_path("out.json", "s1", Path::new("/tmp"));
        assert_eq!(path, PathBuf::from("/tmp/out.json"));
    }

    #[test]
    fn resolve_import_path_requires_argument() {
        assert!(resolve_import_path("", Path::new("/tmp")).is_err());
    }

    #[test]
    fn format_history_v1_shows_role_and_truncated_text() {
        let out = format_history_list(V1_EXPORT).unwrap();
        assert!(out.contains("user"));
        assert!(out.contains("hello there"));
        assert!(out.contains("assistant"));
        assert!(out.contains("hi back"));
    }

    #[test]
    fn format_history_v2_truncates_long_lines() {
        let out = format_history_list(V2_EXPORT).unwrap();
        assert!(out.contains("question"));
        assert!(out.contains('…'), "long assistant text should truncate: {out}");
    }

    #[test]
    fn format_history_empty_export() {
        assert_eq!(format_history_list("[]").unwrap(), "(no messages)");
    }

    #[test]
    fn format_history_rejects_invalid_json() {
        let err = format_history_list("not json").unwrap_err();
        assert!(err.contains("invalid session export"), "{err}");
    }

    #[test]
    fn format_import_summary_counts_roles() {
        let summary = format_import_summary(V1_EXPORT).unwrap();
        assert!(summary.contains("2 messages"));
        assert!(summary.contains("1 user"));
        assert!(summary.contains("1 assistant"));
        assert!(summary.contains("v1"));
    }

    #[test]
    fn export_round_trip_through_temp_file() {
        let fs = MockFs::new();
        let path = PathBuf::from("/tmp/exports/roundtrip.json");
        write_export_file(&path, V1_EXPORT, &fs).unwrap();
        let read_back = fs.read_to_string(&path).unwrap();
        assert_eq!(read_back, V1_EXPORT);
        let summary = format_import_summary(&read_back).unwrap();
        assert!(summary.contains("2 messages"));
        let history = format_history_list(&read_back).unwrap();
        assert!(history.contains("hello there"));
    }

    #[test]
    fn import_summary_rejects_bad_json() {
        let err = format_import_summary("{").unwrap_err();
        assert!(err.contains("invalid transcript"), "{err}");
    }
}
