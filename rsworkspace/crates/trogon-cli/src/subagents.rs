//! Custom subagent definitions loaded from `.claude/agents/`.
//!
//! Each definition is a markdown file with optional YAML-ish frontmatter:
//! ```text
//! ---
//! name: code-reviewer
//! description: Reviews a diff for bugs
//! tools: read_file, grep, git_diff
//! model: claude-sonnet-4-6
//! ---
//! You are a meticulous code reviewer. ...
//! ```
//! Definitions are discovered from `<cwd>/.claude/agents/*.md` (project) and
//! `~/.config/trogon/agents/*.md` (user); project definitions win on name clash.
//! The `/agents` REPL command lists and inspects them.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentDef {
    pub name: String,
    pub description: String,
    /// Tool names the subagent may use (empty = inherit all).
    pub tools: Vec<String>,
    /// Model alias/id override (None = default).
    pub model: Option<String>,
    /// The markdown body — the subagent's system prompt.
    pub system_prompt: String,
    /// File the definition was loaded from.
    pub source: PathBuf,
}

/// Parse one definition file's contents. `name` falls back to the file stem when
/// the frontmatter omits it. Returns `None` only when no usable name can be found.
pub fn parse_subagent(content: &str, source: PathBuf) -> Option<SubagentDef> {
    let mut name: Option<String> = None;
    let mut description = String::new();
    let mut tools = Vec::new();
    let mut model = None;

    let body = if content.trim_start().starts_with("---") {
        let mut iter = content.trim_start().lines();
        iter.next(); // opening ---
        let mut in_frontmatter = true;
        let mut body_lines = Vec::new();
        for line in iter {
            if in_frontmatter && line.trim() == "---" {
                in_frontmatter = false;
                continue;
            }
            if in_frontmatter {
                if let Some((k, v)) = line.split_once(':') {
                    let (k, v) = (k.trim(), v.trim());
                    match k {
                        "name" if !v.is_empty() => name = Some(v.to_string()),
                        "description" => description = v.to_string(),
                        "model" if !v.is_empty() => model = Some(v.to_string()),
                        "tools" => {
                            tools = v
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                        }
                        _ => {}
                    }
                }
            } else {
                body_lines.push(line);
            }
        }
        body_lines.join("\n").trim().to_string()
    } else {
        content.trim().to_string()
    };

    let name = name.or_else(|| {
        source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
    })?;

    Some(SubagentDef {
        name,
        description,
        tools,
        model,
        system_prompt: body,
        source,
    })
}

/// Load all subagent definitions for `cwd`: project (`<cwd>/.claude/agents`) then
/// user (`~/.config/trogon/agents`). Project definitions override user ones on a
/// name clash. Results are sorted by name.
pub fn load_subagents(cwd: &Path) -> Vec<SubagentDef> {
    let mut by_name: std::collections::HashMap<String, SubagentDef> = std::collections::HashMap::new();
    // User first (lower precedence), then project overrides.
    let user_dir = crate::session_store::expand_tilde("~/.config/trogon/agents");
    let project_dir = cwd.join(".claude/agents");
    for dir in [user_dir, project_dir] {
        for def in read_dir_defs(&dir) {
            by_name.insert(def.name.clone(), def);
        }
    }
    let mut defs: Vec<SubagentDef> = by_name.into_values().collect();
    defs.sort_by(|a, b| a.name.cmp(&b.name));
    defs
}

/// Read + parse every `*.md` file in `dir` (non-recursive). Missing dir → empty.
fn read_dir_defs(dir: &Path) -> Vec<SubagentDef> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut defs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path)
            && let Some(def) = parse_subagent(&content, path)
        {
            defs.push(def);
        }
    }
    defs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_body() {
        let content = "---\nname: reviewer\ndescription: Reviews code\ntools: read_file, grep\nmodel: claude-sonnet-4-6\n---\nYou are a reviewer.\nBe thorough.";
        let def = parse_subagent(content, PathBuf::from("/x/reviewer.md")).unwrap();
        assert_eq!(def.name, "reviewer");
        assert_eq!(def.description, "Reviews code");
        assert_eq!(def.tools, vec!["read_file", "grep"]);
        assert_eq!(def.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(def.system_prompt, "You are a reviewer.\nBe thorough.");
    }

    #[test]
    fn name_falls_back_to_file_stem() {
        let def = parse_subagent("Just a body, no frontmatter.", PathBuf::from("/a/planner.md")).unwrap();
        assert_eq!(def.name, "planner");
        assert_eq!(def.system_prompt, "Just a body, no frontmatter.");
        assert!(def.tools.is_empty());
        assert!(def.model.is_none());
    }

    #[test]
    fn frontmatter_only_has_empty_prompt() {
        let def = parse_subagent("---\nname: noop\ndescription: nothing\n---\n", PathBuf::from("/n.md")).unwrap();
        assert_eq!(def.name, "noop");
        assert_eq!(def.system_prompt, "");
    }

    #[test]
    fn empty_tools_and_model_omitted() {
        let def = parse_subagent("---\nname: x\ntools:\nmodel:\n---\nbody", PathBuf::from("/x.md")).unwrap();
        assert!(def.tools.is_empty());
        assert!(def.model.is_none());
    }
}
