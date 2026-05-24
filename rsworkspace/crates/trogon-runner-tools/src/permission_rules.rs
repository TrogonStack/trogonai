//! Static permission rules loaded from `TROGON.md` and `/config`.
//!
//! Rules use key-value syntax (`allow_paths:`, `deny_paths:`, `allow_commands:`,
//! `deny_commands:`). They may appear in a `## Permissions` section, as bare
//! lines at the top of the file, or inside markdown code fences (e.g. yaml
//! examples). Every matching line in the document is collected.
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

/// Tools whose path input field is matched against path rules (canonical names).
const FILE_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "str_replace",
    "glob",
    "grep",
    "search",
    "list_dir",
    "notebook_edit",
    "change_directory",
];

/// Normalize tool names from ACP/agent conventions to canonical names used in rules.
pub fn normalize_tool_name(tool_name: &str) -> &str {
    match tool_name {
        "bash" | "Bash" => "bash",
        "read_file" | "Read" => "read_file",
        "write_file" | "Write" => "write_file",
        "str_replace" | "Edit" => "str_replace",
        "glob" | "Glob" => "glob",
        "grep" | "Grep" | "search" | "search_files" => "grep",
        "list_dir" | "LS" => "list_dir",
        "notebook_edit" | "NotebookEdit" => "notebook_edit",
        "todo_read" => "todo_read",
        "todo_write" => "todo_write",
        "git_status" => "git_status",
        "git_diff" => "git_diff",
        "git_log" => "git_log",
        "git_commit" => "git_commit",
        "change_directory" => "change_directory",
        other => other,
    }
}

/// Extract a filesystem path from tool input (`path`, `file_path`, or `notebook_path`).
pub fn extract_path_from_input(tool_input: &Value) -> Option<&str> {
    ["path", "file_path", "notebook_path"]
        .iter()
        .find_map(|key| tool_input.get(*key).and_then(|v| v.as_str()))
}

fn extract_command_from_input(tool_input: &Value) -> Option<&str> {
    tool_input.get("command").and_then(|v| v.as_str())
}

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

fn parse_rule_line(line: &str, rules: &mut PermissionRules) {
    let trimmed = line.trim();
    if trimmed.starts_with("```") {
        return;
    }

    let Some((key, values)) = trimmed.split_once(':') else {
        return;
    };

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

impl PermissionRules {
    /// Parse rules from the concatenated TROGON.md text.
    ///
    /// Reads `key: value1, value2` lines anywhere in the document (including
    /// inside markdown code fences and bare lines before any heading).
    pub fn parse(text: &str) -> Self {
        let mut rules = Self::default();
        for line in text.lines() {
            parse_rule_line(line, &mut rules);
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
        let normalized = normalize_tool_name(tool_name);

        if FILE_TOOLS.contains(&normalized)
            && let Some(path) = extract_path_from_input(tool_input)
        {
            return self.check_path(path);
        }

        if normalized == "bash"
            && let Some(cmd) = extract_command_from_input(tool_input)
        {
            return self.check_command(cmd);
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
    if patterns.iter().any(|p| p == "*" || p == "**") {
        return true;
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
/// A lone `*` pattern matches any command.
fn matches_any_prefix(command: &str, patterns: &[String]) -> bool {
    let trimmed = command.trim();
    patterns.iter().any(|p| {
        if p == "*" {
            return true;
        }
        trimmed == p.as_str()
            || trimmed.starts_with(&format!("{p} "))
            || trimmed.starts_with(&format!("{p}\n"))
    })
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
    fn parse_rules_after_other_headings() {
        let md = "## Permissions\nallow_paths: src/**\n## Other\nallow_paths: also_parsed\n";
        let r = PermissionRules::parse(md);
        assert_eq!(r.allow_paths, vec!["src/**", "also_parsed"]);
    }

    #[test]
    fn parse_bare_lines_without_section() {
        let md = "allow_paths: **\nallow_commands: *\n# Just some text\n";
        let r = PermissionRules::parse(md);
        assert_eq!(r.allow_paths, vec!["**"]);
        assert_eq!(r.allow_commands, vec!["*"]);
    }

    #[test]
    fn parse_rules_inside_yaml_fence() {
        let md = "```yaml\nallow_paths: src/**\nallow_commands: cargo test\n```\n";
        let r = PermissionRules::parse(md);
        assert_eq!(r.allow_paths, vec!["src/**"]);
        assert_eq!(r.allow_commands, vec!["cargo test"]);
    }

    #[test]
    fn parse_no_permission_lines_returns_empty() {
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

    #[test]
    fn allow_commands_wildcard_matches_any_command() {
        let r = PermissionRules::parse("allow_commands: *\n");
        assert_eq!(
            r.check("bash", &serde_json::json!({"command": "bash"})),
            RuleDecision::Allow
        );
        assert_eq!(
            r.check("bash", &serde_json::json!({"command": "pwd"})),
            RuleDecision::Allow
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

    #[test]
    fn read_with_file_path_matches_allow_paths_wildcard() {
        let r = PermissionRules::parse("allow_paths: **\n");
        assert_eq!(
            r.check("Read", &serde_json::json!({"file_path": "/home/user/file.txt"})),
            RuleDecision::Allow
        );
    }

    #[test]
    fn bash_with_allow_commands_wildcard() {
        let r = PermissionRules::parse("allow_commands: *\n");
        assert_eq!(
            r.check("Bash", &serde_json::json!({"command": "pwd"})),
            RuleDecision::Allow
        );
    }

    #[test]
    fn edit_with_file_path_matches_deny_paths() {
        let r = PermissionRules::parse("deny_paths: .env\n");
        assert_eq!(
            r.check("Edit", &serde_json::json!({"file_path": ".env"})),
            RuleDecision::Deny
        );
    }

    #[test]
    fn notebook_edit_with_notebook_path() {
        let r = PermissionRules::parse("allow_paths: notebooks/**\n");
        assert_eq!(
            r.check(
                "NotebookEdit",
                &serde_json::json!({"notebook_path": "notebooks/test.ipynb"}),
            ),
            RuleDecision::Allow
        );
    }
}
