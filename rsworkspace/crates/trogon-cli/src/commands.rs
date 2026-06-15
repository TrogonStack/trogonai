//! Custom slash commands loaded from `.claude/commands/*.md`.
//!
//! Discovery + frontmatter parsing + template substitution live here so a future
//! Skills loader (SKILL-1) can reuse the same skeleton: scan markdown files under
//! a config root, parse YAML frontmatter, and dispatch by name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// A user-defined slash command discovered from markdown on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomCommand {
    /// Slash name without leading `/` (e.g. `git:commit`).
    pub name: String,
    pub description: String,
    /// Prompt template body (after frontmatter).
    pub body: String,
    /// Optional model override from frontmatter (applied by the REPL for that invocation).
    pub model: Option<String>,
    /// Optional tool allow-list from frontmatter (applied per turn via prompt `_meta`).
    pub allowed_tools: Vec<String>,
    pub source: PathBuf,
}

/// Expanded invocation ready to submit as a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomCommandDispatch {
    pub prompt: String,
    pub model: Option<String>,
    pub allowed_tools: Vec<String>,
}

/// In-memory index of loaded custom commands, keyed by slash name (no `/`).
#[derive(Debug, Clone, Default)]
pub struct CustomCommandRegistry {
    by_name: HashMap<String, CustomCommand>,
}

impl CustomCommandRegistry {
    pub fn new(commands: impl IntoIterator<Item = CustomCommand>) -> Self {
        let mut by_name = HashMap::new();
        for cmd in commands {
            by_name.insert(cmd.name.clone(), cmd);
        }
        Self { by_name }
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn commands(&self) -> impl Iterator<Item = &CustomCommand> {
        let mut names: Vec<_> = self.by_name.keys().collect();
        names.sort();
        names.into_iter().filter_map(|n| self.by_name.get(n))
    }

    pub fn get(&self, slash_name: &str) -> Option<&CustomCommand> {
        let key = slash_name.strip_prefix('/').unwrap_or(slash_name);
        self.by_name.get(key)
    }

    /// Resolve `/name [args]` to an expanded prompt, or `None` when unknown.
    pub fn dispatch(&self, cmd: &str, args: &str) -> Option<CustomCommandDispatch> {
        let def = self.get(cmd)?;
        Some(CustomCommandDispatch {
            prompt: substitute_args(&def.body, args),
            model: def.model.clone(),
            allowed_tools: def.allowed_tools.clone(),
        })
    }
}

/// Map Claude-style custom-command tool names to trogon runner tool ids.
pub fn normalize_allowed_tools_for_runner(tools: &[String]) -> Vec<String> {
    tools
        .iter()
        .map(|t| {
            let base = t.split('(').next().unwrap_or(t).trim();
            trogon_runner_tools::permission_rules::normalize_tool_name(base).to_string()
        })
        .collect()
}

/// Per-turn prompt overrides derived from a custom-command dispatch.
pub fn prompt_opts_from_dispatch(dispatch: &CustomCommandDispatch) -> crate::session::PromptOpts {
    crate::session::PromptOpts {
        tool_allowlist: normalize_allowed_tools_for_runner(&dispatch.allowed_tools),
    }
}

/// Load project-level (`.claude/commands/`) then user-level (`~/.claude/commands/`).
/// Project definitions override user definitions on a name clash.
pub fn load_commands(project_dir: &Path) -> CustomCommandRegistry {
    let mut by_name: HashMap<String, CustomCommand> = HashMap::new();
    for dir in command_roots(project_dir) {
        for cmd in discover_in_root(&dir) {
            by_name.insert(cmd.name.clone(), cmd);
        }
    }
    CustomCommandRegistry::new(by_name.into_values())
}

/// Format a help section listing loaded custom commands (empty when none).
pub fn format_custom_commands_help(registry: &CustomCommandRegistry) -> String {
    if registry.is_empty() {
        return String::new();
    }
    let m = "\x1b[35m";
    let r = "\x1b[0m";
    let dim = "\x1b[2m";
    let mut out = format!("\nCustom commands ({m}.claude/commands/{r}):\n");
    for cmd in registry.commands() {
        let desc = if cmd.description.is_empty() {
            String::new()
        } else {
            format!("{dim}{}{r}", cmd.description)
        };
        out.push_str(&format!("  {m}/{}{r}  {desc}\n", cmd.name));
    }
    out
}

fn command_roots(project_dir: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        roots.push(PathBuf::from(home).join(".claude/commands"));
    }
    roots.push(project_dir.join(".claude/commands"));
    roots
}

fn discover_in_root(root: &Path) -> Vec<CustomCommand> {
    let mut found = Vec::new();
    collect_commands(root, root, &mut found);
    found
}

fn collect_commands(base: &Path, dir: &Path, out: &mut Vec<CustomCommand>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_commands(base, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md")
            && let Ok(content) = std::fs::read_to_string(&path)
            && let Some(cmd) = parse_command_file(&content, &path, base)
        {
            out.push(cmd);
        }
    }
}

/// Derive the slash command name from a file path under a commands root.
/// `.claude/commands/git/commit.md` → `git:commit`.
pub fn command_name_from_path(base: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(base).ok()?;
    let rel = rel.with_extension("");
    let name = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join(":");
    if name.is_empty() { None } else { Some(name) }
}

#[derive(Debug, Default, Deserialize)]
struct CommandFrontmatter {
    description: Option<String>,
    model: Option<String>,
    #[serde(rename = "allowed-tools", default)]
    allowed_tools: Option<AllowedToolsField>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AllowedToolsField {
    List(Vec<String>),
    Single(String),
}

impl AllowedToolsField {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::List(items) => items,
            Self::Single(s) => s
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect(),
        }
    }
}

/// Parse one command markdown file: YAML frontmatter + body template.
pub fn parse_command_file(content: &str, source: &Path, base: &Path) -> Option<CustomCommand> {
    let name = command_name_from_path(base, source)?;
    let (frontmatter, body) = split_frontmatter(content)?;
    let meta: CommandFrontmatter = serde_yaml::from_str(frontmatter).ok()?;
    let allowed_tools = meta.allowed_tools.map(AllowedToolsField::into_vec).unwrap_or_default();
    Some(CustomCommand {
        name,
        description: meta.description.unwrap_or_default(),
        body: body.trim().to_string(),
        model: meta.model.filter(|m| !m.is_empty()),
        allowed_tools,
        source: source.to_path_buf(),
    })
}

fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Some(("", content));
    }
    let rest = trimmed.strip_prefix("---")?;
    let rest = rest.strip_prefix('\n').or_else(|| rest.strip_prefix("\r\n"))?;
    let end = rest.find("\n---")?;
    let (yaml, body) = rest.split_at(end);
    let body = body.strip_prefix("\n---").unwrap_or(body);
    let body = body.strip_prefix('\n').or_else(|| body.strip_prefix("\r\n")).unwrap_or(body);
    Some((yaml, body))
}

/// Replace `$ARGUMENTS` and positional `$1`, `$2`, … in a command template.
pub fn substitute_args(template: &str, args: &str) -> String {
    let trimmed = args.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    let mut out = template.to_string();
    for i in (1..=parts.len()).rev() {
        out = out.replace(&format!("${i}"), parts[i - 1]);
    }
    out.replace("$ARGUMENTS", trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn command_name_from_path_handles_namespacing() {
        let base = Path::new("/proj/.claude/commands");
        assert_eq!(
            command_name_from_path(base, Path::new("/proj/.claude/commands/git/commit.md")).as_deref(),
            Some("git:commit")
        );
        assert_eq!(
            command_name_from_path(base, Path::new("/proj/.claude/commands/review.md")).as_deref(),
            Some("review")
        );
    }

    #[test]
    fn parses_frontmatter_with_serde_yaml() {
        let content = "---\ndescription: Commit work\nmodel: claude-sonnet-4-6\nallowed-tools: Read, Bash\n---\nCommit: $ARGUMENTS\n";
        let cmd = parse_command_file(
            content,
            Path::new("/p/.claude/commands/git/commit.md"),
            Path::new("/p/.claude/commands"),
        )
        .unwrap();
        assert_eq!(cmd.name, "git:commit");
        assert_eq!(cmd.description, "Commit work");
        assert_eq!(cmd.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(cmd.allowed_tools, vec!["Read", "Bash"]);
        assert_eq!(cmd.body, "Commit: $ARGUMENTS");
    }

    #[test]
    fn parses_allowed_tools_yaml_list() {
        let content = "---\nallowed-tools:\n  - Read\n  - Bash(git:*)\n---\nbody\n";
        let cmd = parse_command_file(
            content,
            Path::new("/p/.claude/commands/x.md"),
            Path::new("/p/.claude/commands"),
        )
        .unwrap();
        assert_eq!(cmd.allowed_tools, vec!["Read", "Bash(git:*)"]);
    }

    #[test]
    fn substitute_arguments_and_positional() {
        assert_eq!(
            substitute_args("All: $ARGUMENTS", "hello world"),
            "All: hello world"
        );
        assert_eq!(
            substitute_args("First=$1 second=$2 rest=$ARGUMENTS", "a b c d"),
            "First=a second=b rest=a b c d"
        );
        assert_eq!(substitute_args("No args: $1", ""), "No args: $1");
    }

    #[test]
    fn discovery_finds_project_user_and_namespaced_commands() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let home = tmp.path().join("home");
        fs::create_dir_all(project.join(".claude/commands/git")).unwrap();
        fs::create_dir_all(home.join(".claude/commands")).unwrap();

        fs::write(
            project.join(".claude/commands/git/commit.md"),
            "---\ndescription: Commit\n---\nCommit $ARGUMENTS\n",
        )
        .unwrap();
        fs::write(
            project.join(".claude/commands/review.md"),
            "---\ndescription: Review\n---\nReview\n",
        )
        .unwrap();
        fs::write(
            home.join(".claude/commands/global.md"),
            "---\ndescription: Global\n---\nGlobal\n",
        )
        .unwrap();

        // Simulate load with explicit roots via discover_in_root (avoid HOME mutation).
        let mut by_name = HashMap::new();
        for dir in [home.join(".claude/commands"), project.join(".claude/commands")] {
            for cmd in discover_in_root(&dir) {
                by_name.insert(cmd.name.clone(), cmd);
            }
        }
        let registry = CustomCommandRegistry::new(by_name.into_values());

        assert!(registry.get("/git:commit").is_some());
        assert!(registry.get("/review").is_some());
        assert!(registry.get("/global").is_some());
        assert_eq!(registry.commands().count(), 3);
    }

    #[test]
    fn project_overrides_user_on_name_clash() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let home = tmp.path().join("home");
        fs::create_dir_all(project.join(".claude/commands")).unwrap();
        fs::create_dir_all(home.join(".claude/commands")).unwrap();
        fs::write(home.join(".claude/commands/dup.md"), "---\ndescription: user\n---\nuser\n").unwrap();
        fs::write(project.join(".claude/commands/dup.md"), "---\ndescription: project\n---\nproject\n")
            .unwrap();

        let mut by_name = HashMap::new();
        for dir in [home.join(".claude/commands"), project.join(".claude/commands")] {
            for cmd in discover_in_root(&dir) {
                by_name.insert(cmd.name.clone(), cmd);
            }
        }
        let registry = CustomCommandRegistry::new(by_name.into_values());
        assert_eq!(registry.get("/dup").unwrap().description, "project");
    }

    #[test]
    fn dispatch_expands_template_and_unknown_falls_through() {
        let registry = CustomCommandRegistry::new([CustomCommand {
            name: "test".into(),
            description: String::new(),
            body: "Run: $ARGUMENTS ($1)".into(),
            model: None,
            allowed_tools: Vec::new(),
            source: PathBuf::from("/x.md"),
        }]);
        let disp = registry.dispatch("/test", "one two").unwrap();
        assert_eq!(disp.prompt, "Run: one two (one)");
        assert!(registry.dispatch("/nope", "").is_none());
    }

    #[test]
    fn normalize_allowed_tools_maps_claude_names() {
        assert_eq!(
            normalize_allowed_tools_for_runner(&["Read".into(), "Bash(git:*)".into()]),
            vec!["read_file", "bash"]
        );
    }

    #[test]
    fn prompt_opts_from_dispatch_normalizes_allowed_tools() {
        let dispatch = CustomCommandDispatch {
            prompt: "do it".into(),
            model: Some("claude-opus-4-7".into()),
            allowed_tools: vec!["Read".into(), "Bash".into()],
        };
        let opts = prompt_opts_from_dispatch(&dispatch);
        assert_eq!(opts.tool_allowlist, vec!["read_file", "bash"]);
    }

    #[test]
    fn format_custom_commands_help_lists_names() {
        let registry = CustomCommandRegistry::new([CustomCommand {
            name: "git:commit".into(),
            description: "Commit changes".into(),
            body: String::new(),
            model: None,
            allowed_tools: Vec::new(),
            source: PathBuf::from("/x.md"),
        }]);
        let help = format_custom_commands_help(&registry);
        assert!(help.contains("/git:commit"));
        assert!(help.contains("Commit changes"));
    }
}
