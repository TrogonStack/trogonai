//! Claude Code-style `settings.json` with a layered hierarchy.
//!
//! Layers, lowest to highest precedence:
//!   1. user:    `~/.config/trogon/settings.json`
//!   2. project: `<cwd>/.claude/settings.json`
//!   3. local:   `<cwd>/.claude/settings.local.json`  (gitignored personal overrides)
//!
//! List fields (`permissions.allow/deny/ask/additionalDirectories`) are UNIONed
//! across layers; scalar fields (`defaultMode`, `cleanupPeriodDays`, `model`) take
//! the highest-precedence value that is set. `permissions.allow/deny` are
//! translated to trogon's rule-text format and pushed to the runner.

use crate::fs::Fs;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub permissions: PermissionsSettings,
    /// Delete sessions whose last activity is older than this many days.
    pub cleanup_period_days: Option<u64>,
    /// Default model alias/id for new sessions.
    pub model: Option<String>,
    /// Extra environment variables (loaded for completeness; not auto-applied).
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct PermissionsSettings {
    /// Rules to auto-allow, Claude-style: `Bash(cmd:*)`, `Read(path)`, …
    pub allow: Vec<String>,
    /// Rules to deny.
    pub deny: Vec<String>,
    /// Rules that always require approval (loaded; default behavior already asks).
    pub ask: Vec<String>,
    /// Directories outside cwd where reads are auto-allowed.
    pub additional_directories: Vec<String>,
    /// Startup permission mode (default/acceptEdits/plan/auto/dontAsk/bypassPermissions).
    pub default_mode: Option<String>,
}

impl Settings {
    /// Load and merge the user → project → local layers for `cwd`.
    pub fn load<F: Fs>(fs: &F, cwd: &Path) -> Settings {
        let mut merged = Settings::default();
        for path in Self::source_paths(cwd) {
            if let Some(layer) = Self::read_layer(fs, &path) {
                merged.merge(layer);
            }
        }
        merged
    }

    fn source_paths(cwd: &Path) -> Vec<PathBuf> {
        vec![
            crate::session_store::expand_tilde("~/.config/trogon/settings.json"),
            cwd.join(".claude/settings.json"),
            cwd.join(".claude/settings.local.json"),
        ]
    }

    fn read_layer<F: Fs>(fs: &F, path: &Path) -> Option<Settings> {
        let raw = fs.read_to_string(path).ok()?;
        if raw.trim().is_empty() {
            return None;
        }
        match serde_json::from_str(&raw) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("warning: {} is invalid ({e}) — ignoring", path.display());
                None
            }
        }
    }

    /// Merge `other` (higher precedence) into `self`.
    fn merge(&mut self, other: Settings) {
        self.permissions.allow.extend(other.permissions.allow);
        self.permissions.deny.extend(other.permissions.deny);
        self.permissions.ask.extend(other.permissions.ask);
        self.permissions
            .additional_directories
            .extend(other.permissions.additional_directories);
        if other.permissions.default_mode.is_some() {
            self.permissions.default_mode = other.permissions.default_mode;
        }
        if other.cleanup_period_days.is_some() {
            self.cleanup_period_days = other.cleanup_period_days;
        }
        if other.model.is_some() {
            self.model = other.model;
        }
        self.env.extend(other.env);
    }

    /// Translate `permissions.allow`/`deny` into trogon rule-text (the format
    /// `PermissionRules::parse` understands), or `None` when there are no
    /// translatable rules. Pushed to the runner via `set_session_config_option`.
    pub fn permission_rules_text(&self) -> Option<String> {
        let mut allow_paths = Vec::new();
        let mut allow_commands = Vec::new();
        let mut deny_paths = Vec::new();
        let mut deny_commands = Vec::new();
        for entry in &self.permissions.allow {
            match translate_rule(entry) {
                Some((RuleKind::Path, v)) => allow_paths.push(v),
                Some((RuleKind::Command, v)) => allow_commands.push(v),
                None => {}
            }
        }
        for entry in &self.permissions.deny {
            match translate_rule(entry) {
                Some((RuleKind::Path, v)) => deny_paths.push(v),
                Some((RuleKind::Command, v)) => deny_commands.push(v),
                None => {}
            }
        }
        let mut lines = Vec::new();
        let mut push = |key: &str, vals: &[String]| {
            if !vals.is_empty() {
                lines.push(format!("{key}: {}", vals.join(", ")));
            }
        };
        push("allow_paths", &allow_paths);
        push("allow_commands", &allow_commands);
        push("deny_paths", &deny_paths);
        push("deny_commands", &deny_commands);
        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }
}

enum RuleKind {
    Path,
    Command,
}

/// Translate one Claude-style rule (`Tool(specifier)`) into a trogon rule value.
/// `Bash(...)` → command; `Read/Edit/Write/MultiEdit(...)` → path. Trailing `:*`
/// wildcards are stripped. Bare tool rules (no specifier) aren't representable.
fn translate_rule(entry: &str) -> Option<(RuleKind, String)> {
    let entry = entry.trim();
    let (tool, arg) = match entry.split_once('(') {
        Some((t, rest)) => (t.trim(), rest.strip_suffix(')').unwrap_or(rest).trim()),
        None => return None,
    };
    let arg = arg.trim().trim_end_matches(":*").trim();
    if arg.is_empty() {
        return None;
    }
    match tool {
        "Bash" => Some((RuleKind::Command, arg.to_string())),
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            Some((RuleKind::Path, arg.to_string()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::mock::MockFs;

    #[test]
    fn missing_files_yield_default_settings() {
        let fs = MockFs::new();
        let s = Settings::load(&fs, Path::new("/proj"));
        assert_eq!(s, Settings::default());
        assert!(s.cleanup_period_days.is_none());
    }

    #[test]
    fn local_overrides_project_overrides_user_for_scalars() {
        let fs = MockFs::new();
        fs.write(
            &crate::session_store::expand_tilde("~/.config/trogon/settings.json"),
            br#"{"cleanupPeriodDays": 30, "permissions": {"defaultMode": "default"}}"#,
        )
        .unwrap();
        fs.write(
            Path::new("/proj/.claude/settings.json"),
            br#"{"cleanupPeriodDays": 14, "permissions": {"defaultMode": "acceptEdits"}}"#,
        )
        .unwrap();
        fs.write(
            Path::new("/proj/.claude/settings.local.json"),
            br#"{"permissions": {"defaultMode": "plan"}}"#,
        )
        .unwrap();
        let s = Settings::load(&fs, Path::new("/proj"));
        assert_eq!(s.cleanup_period_days, Some(14)); // project wins (local didn't set)
        assert_eq!(s.permissions.default_mode.as_deref(), Some("plan")); // local wins
    }

    #[test]
    fn list_fields_union_across_layers() {
        let fs = MockFs::new();
        fs.write(
            Path::new("/proj/.claude/settings.json"),
            br#"{"permissions": {"allow": ["Bash(cargo test:*)"], "additionalDirectories": ["/shared"]}}"#,
        )
        .unwrap();
        fs.write(
            Path::new("/proj/.claude/settings.local.json"),
            br#"{"permissions": {"allow": ["Read(/tmp/**)"], "additionalDirectories": ["/extra"]}}"#,
        )
        .unwrap();
        let s = Settings::load(&fs, Path::new("/proj"));
        assert_eq!(s.permissions.allow.len(), 2);
        assert_eq!(
            s.permissions.additional_directories,
            vec!["/shared".to_string(), "/extra".to_string()]
        );
    }

    #[test]
    fn translates_allow_deny_to_rule_text() {
        let mut s = Settings::default();
        s.permissions.allow = vec!["Bash(cargo test:*)".into(), "Read(src/**)".into()];
        s.permissions.deny = vec!["Bash(rm -rf:*)".into(), "Edit(.env)".into()];
        let text = s.permission_rules_text().unwrap();
        assert!(text.contains("allow_commands: cargo test"));
        assert!(text.contains("allow_paths: src/**"));
        assert!(text.contains("deny_commands: rm -rf"));
        assert!(text.contains("deny_paths: .env"));
    }

    #[test]
    fn no_rules_yields_none() {
        assert!(Settings::default().permission_rules_text().is_none());
        // Bare/untranslatable rules are skipped.
        let mut s = Settings::default();
        s.permissions.allow = vec!["WebFetch".into(), "Unknown(x)".into()];
        assert!(s.permission_rules_text().is_none());
    }

    #[test]
    fn invalid_json_layer_is_ignored() {
        let fs = MockFs::new();
        fs.write(Path::new("/proj/.claude/settings.json"), b"{not json").unwrap();
        // Should not panic; returns defaults.
        let s = Settings::load(&fs, Path::new("/proj"));
        assert_eq!(s, Settings::default());
    }
}
