//! Static permission rules loaded from `TROGON.md` and `/config`.
//!
//! Rules are defined in a `## Permissions` section using key-value syntax:
//!
//! ```markdown
//! ## Permissions
//!
//! allow_paths: src/**, tests/**
//! deny_paths: .env, **/.env, secrets/**
//! allow_commands: cargo test, cargo build, cargo check
//! deny_commands: rm -rf, sudo, git push --force
//! ```
//!
//! Evaluation order per tool call:
//!
//! 1. If any **deny** rule matches → `Deny` (no interactive prompt).
//! 2. If any **allow** rule matches → `Allow` (auto-approve, no prompt).
//! 3. Otherwise → `Ask` (fall through to the interactive permission gate).

use globset::{Glob, GlobSetBuilder};
use serde_json::Value;

/// Tools whose `path` input field is matched against path rules.
const FILE_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "str_replace",
    "glob",
    "list_dir",
    "notebook_edit",
];

/// Result of evaluating the static rules against a single tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleDecision {
    /// A deny rule matched — reject without asking.
    Deny,
    /// An allow rule matched — approve without asking.
    Allow,
    /// No rule matched — ask the user interactively.
    Ask,
}

/// Static permission rules parsed from `TROGON.md`.
#[derive(Debug, Default, Clone)]
pub struct PermissionRules {
    allow_paths: Vec<String>,
    deny_paths: Vec<String>,
    allow_commands: Vec<String>,
    deny_commands: Vec<String>,
}

impl PermissionRules {
    /// Parse rules from the concatenated TROGON.md text.
    ///
    /// Looks for a `## Permissions` section and reads `key: value1, value2`
    /// lines within it. Stops at the next `##` heading or end of file.
    pub fn parse(text: &str) -> Self {
        let mut rules = Self::default();
        let mut in_section = false;

        for line in text.lines() {
            let trimmed = line.trim();

            if trimmed.starts_with("## ") {
                in_section = trimmed.eq_ignore_ascii_case("## permissions")
                    || trimmed.eq_ignore_ascii_case("## Permissions");
                continue;
            }

            if !in_section {
                continue;
            }

            // Parse `key: v1, v2, v3` lines.
            if let Some((key, values)) = trimmed.split_once(':') {
                let items: Vec<String> = values
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                match key.trim() {
                    "allow_paths" => rules.allow_paths.extend(items),
                    "deny_paths" => rules.deny_paths.extend(items),
                    "allow_commands" => rules.allow_commands.extend(items),
                    "deny_commands" => rules.deny_commands.extend(items),
                    _ => {}
                }
            }
        }

        rules
    }

    /// Merge another set of rules into this one (additive — no deduplication).
    pub fn merge(&mut self, other: PermissionRules) {
        self.allow_paths.extend(other.allow_paths);
        self.deny_paths.extend(other.deny_paths);
        self.allow_commands.extend(other.allow_commands);
        self.deny_commands.extend(other.deny_commands);
    }

    /// Evaluate rules for a single tool call.
    pub fn check(&self, tool_name: &str, tool_input: &Value) -> RuleDecision {
        if FILE_TOOLS.contains(&tool_name) {
            if let Some(path) = tool_input["path"].as_str() {
                return self.check_path(path);
            }
        }

        if tool_name == "bash" {
            if let Some(cmd) = tool_input["command"].as_str() {
                return self.check_command(cmd);
            }
        }

        RuleDecision::Ask
    }

    fn check_path(&self, path: &str) -> RuleDecision {
        // Deny beats allow.
        if matches_any_glob(path, &self.deny_paths) {
            return RuleDecision::Deny;
        }
        if matches_any_glob(path, &self.allow_paths) {
            return RuleDecision::Allow;
        }
        RuleDecision::Ask
    }

    fn check_command(&self, command: &str) -> RuleDecision {
        // Deny beats allow.
        if matches_any_prefix(command, &self.deny_commands) {
            return RuleDecision::Deny;
        }
        if matches_any_prefix(command, &self.allow_commands) {
            return RuleDecision::Allow;
        }
        RuleDecision::Ask
    }

    pub fn is_empty(&self) -> bool {
        self.allow_paths.is_empty()
            && self.deny_paths.is_empty()
            && self.allow_commands.is_empty()
            && self.deny_commands.is_empty()
    }
}

/// Match `path` against any of the glob patterns. Invalid patterns are skipped.
fn matches_any_glob(path: &str, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        if let Ok(g) = Glob::new(pat) {
            builder.add(g);
        }
    }
    builder
        .build()
        .map(|set| set.is_match(path))
        .unwrap_or(false)
}

/// Match `command` against any of the prefix patterns (case-sensitive).
fn matches_any_prefix(command: &str, patterns: &[String]) -> bool {
    let trimmed = command.trim();
    patterns
        .iter()
        .any(|p| trimmed == p.as_str() || trimmed.starts_with(&format!("{p} ")) || trimmed.starts_with(&format!("{p}\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse ─────────────────────────────────────────────────────────────────

    #[test]
    fn parse_allow_paths() {
        let md = "## Permissions\nallow_paths: src/**, tests/**\n";
        let r = PermissionRules::parse(md);
        assert_eq!(r.allow_paths, vec!["src/**", "tests/**"]);
    }

    #[test]
    fn parse_deny_paths() {
        let md = "## Permissions\ndeny_paths: .env, secrets/**\n";
        let r = PermissionRules::parse(md);
        assert_eq!(r.deny_paths, vec![".env", "secrets/**"]);
    }

    #[test]
    fn parse_allow_commands() {
        let md = "## Permissions\nallow_commands: cargo test, cargo build\n";
        let r = PermissionRules::parse(md);
        assert_eq!(r.allow_commands, vec!["cargo test", "cargo build"]);
    }

    #[test]
    fn parse_deny_commands() {
        let md = "## Permissions\ndeny_commands: rm -rf, sudo\n";
        let r = PermissionRules::parse(md);
        assert_eq!(r.deny_commands, vec!["rm -rf", "sudo"]);
    }

    #[test]
    fn parse_stops_at_next_heading() {
        let md = "## Permissions\nallow_paths: src/**\n## Other\nallow_paths: should_not_appear\n";
        let r = PermissionRules::parse(md);
        assert_eq!(r.allow_paths, vec!["src/**"]);
    }

    #[test]
    fn parse_no_section_returns_empty() {
        let r = PermissionRules::parse("# Just some text\nno permissions here");
        assert!(r.is_empty());
    }

    #[test]
    fn parse_case_insensitive_heading() {
        let md = "## PERMISSIONS\nallow_paths: src/**\n";
        let r = PermissionRules::parse(md);
        assert_eq!(r.allow_paths, vec!["src/**"]);
    }

    #[test]
    fn parse_full_example() {
        let md = "\
# Project

## Permissions

allow_paths: src/**, tests/**
deny_paths: .env, **/.env
allow_commands: cargo test, cargo build
deny_commands: rm -rf, sudo

## Other section
";
        let r = PermissionRules::parse(md);
        assert_eq!(r.allow_paths, vec!["src/**", "tests/**"]);
        assert_eq!(r.deny_paths, vec![".env", "**/.env"]);
        assert_eq!(r.allow_commands, vec!["cargo test", "cargo build"]);
        assert_eq!(r.deny_commands, vec!["rm -rf", "sudo"]);
    }

    // ── check — paths ─────────────────────────────────────────────────────────

    #[test]
    fn allow_path_matches_glob() {
        let r = PermissionRules::parse("## Permissions\nallow_paths: src/**\n");
        assert_eq!(
            r.check("read_file", &serde_json::json!({"path": "src/main.rs"})),
            RuleDecision::Allow
        );
    }

    #[test]
    fn deny_path_beats_allow_path() {
        let r = PermissionRules::parse(
            "## Permissions\nallow_paths: src/**\ndeny_paths: src/.env\n",
        );
        assert_eq!(
            r.check("write_file", &serde_json::json!({"path": "src/.env"})),
            RuleDecision::Deny
        );
    }

    #[test]
    fn unmatched_path_returns_ask() {
        let r = PermissionRules::parse("## Permissions\nallow_paths: src/**\n");
        assert_eq!(
            r.check("read_file", &serde_json::json!({"path": "vendor/lib.rs"})),
            RuleDecision::Ask
        );
    }

    #[test]
    fn dotenv_deny_pattern() {
        let r = PermissionRules::parse("## Permissions\ndeny_paths: .env, **/.env\n");
        assert_eq!(
            r.check("read_file", &serde_json::json!({"path": ".env"})),
            RuleDecision::Deny
        );
        assert_eq!(
            r.check("read_file", &serde_json::json!({"path": "config/.env"})),
            RuleDecision::Deny
        );
    }

    // ── check — commands ──────────────────────────────────────────────────────

    #[test]
    fn allow_command_prefix_match() {
        let r = PermissionRules::parse("## Permissions\nallow_commands: cargo test\n");
        assert_eq!(
            r.check("bash", &serde_json::json!({"command": "cargo test --all"})),
            RuleDecision::Allow
        );
    }

    #[test]
    fn allow_command_exact_match() {
        let r = PermissionRules::parse("## Permissions\nallow_commands: cargo test\n");
        assert_eq!(
            r.check("bash", &serde_json::json!({"command": "cargo test"})),
            RuleDecision::Allow
        );
    }

    #[test]
    fn deny_command_beats_allow_command() {
        let r = PermissionRules::parse(
            "## Permissions\nallow_commands: cargo\ndeny_commands: cargo publish\n",
        );
        assert_eq!(
            r.check("bash", &serde_json::json!({"command": "cargo publish"})),
            RuleDecision::Deny
        );
    }

    #[test]
    fn unmatched_command_returns_ask() {
        let r = PermissionRules::parse("## Permissions\nallow_commands: cargo test\n");
        assert_eq!(
            r.check("bash", &serde_json::json!({"command": "make build"})),
            RuleDecision::Ask
        );
    }

    #[test]
    fn rm_rf_deny() {
        let r = PermissionRules::parse("## Permissions\ndeny_commands: rm -rf\n");
        assert_eq!(
            r.check("bash", &serde_json::json!({"command": "rm -rf /"})),
            RuleDecision::Deny
        );
    }

    // ── check — no path/command field returns Ask ─────────────────────────────

    #[test]
    fn file_tool_without_path_field_returns_ask() {
        let r = PermissionRules::parse("## Permissions\nallow_paths: src/**\n");
        assert_eq!(
            r.check("read_file", &serde_json::json!({})),
            RuleDecision::Ask
        );
    }

    #[test]
    fn bash_without_command_field_returns_ask() {
        let r = PermissionRules::parse("## Permissions\ndeny_commands: rm -rf\n");
        assert_eq!(r.check("bash", &serde_json::json!({})), RuleDecision::Ask);
    }

    #[test]
    fn unknown_tool_returns_ask() {
        let r = PermissionRules::parse("## Permissions\nallow_paths: src/**\n");
        assert_eq!(
            r.check("some_other_tool", &serde_json::json!({"path": "src/x.rs"})),
            RuleDecision::Ask
        );
    }

    // ── merge ─────────────────────────────────────────────────────────────────

    #[test]
    fn merge_combines_rules() {
        let mut a = PermissionRules::parse("## Permissions\nallow_paths: src/**\n");
        let b = PermissionRules::parse("## Permissions\ndeny_commands: rm -rf\n");
        a.merge(b);
        assert_eq!(a.allow_paths, vec!["src/**"]);
        assert_eq!(a.deny_commands, vec!["rm -rf"]);
    }
}
