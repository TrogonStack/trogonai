//! Local `SKILL.md` filesystem runtime: discover skills on disk and dispatch
//! their bodies as prompt templates (with `$ARGUMENTS` substitution).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::md_template::{split_frontmatter, substitute_args};

/// A skill discovered from a `SKILL.md` file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Prompt template body (after frontmatter).
    pub body: String,
    pub source: PathBuf,
}

/// In-memory index of loaded skills, keyed by name.
#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    by_name: HashMap<String, Skill>,
}

impl SkillRegistry {
    pub fn new(skills: impl IntoIterator<Item = Skill>) -> Self {
        let mut by_name = HashMap::new();
        for skill in skills {
            by_name.insert(skill.name.clone(), skill);
        }
        Self { by_name }
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn skills(&self) -> impl Iterator<Item = &Skill> {
        let mut names: Vec<_> = self.by_name.keys().collect();
        names.sort();
        names.into_iter().filter_map(|n| self.by_name.get(n))
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.by_name.get(name)
    }

    /// Resolve `name [args]` to an expanded prompt body, or `None` when unknown.
    pub fn dispatch(&self, name: &str, args: &str) -> Option<String> {
        let def = self.get(name)?;
        Some(substitute_args(&def.body, args))
    }
}

/// Load user-level then project-level skill roots. Later roots override earlier
/// definitions on a name clash.
pub fn load_skills(project_dir: &Path) -> SkillRegistry {
    let mut by_name: HashMap<String, Skill> = HashMap::new();
    for root in skill_roots(project_dir) {
        for skill in discover_in_root(&root) {
            by_name.insert(skill.name.clone(), skill);
        }
    }
    SkillRegistry::new(by_name.into_values())
}

/// Format a listing of discovered skills (empty when none).
pub fn format_skills_list(registry: &SkillRegistry) -> String {
    if registry.is_empty() {
        return "no skills found — add SKILL.md under ~/.config/trogon/skills/<name>/ or .trogon/skills/<name>/".to_string();
    }
    let mut out = String::from("skills:\n");
    for skill in registry.skills() {
        let desc = if skill.description.is_empty() {
            String::new()
        } else {
            format!("  {}", skill.description)
        };
        out.push_str(&format!("  {:<20}{desc}\n", skill.name));
    }
    out.push_str("\nuse /skill <name> [args] to run a skill");
    out
}

/// Format a help section listing loaded skills (empty when none).
pub fn format_skills_help(registry: &SkillRegistry) -> String {
    if registry.is_empty() {
        return String::new();
    }
    let m = "\x1b[35m";
    let r = "\x1b[0m";
    let dim = "\x1b[2m";
    let mut out = format!("\nSkills ({m}SKILL.md{r}):\n");
    for skill in registry.skills() {
        let desc = if skill.description.is_empty() {
            String::new()
        } else {
            format!("{dim}{}{r}", skill.description)
        };
        out.push_str(&format!("  {m}/skill{r} {}{desc}\n", skill.name));
    }
    out
}

fn skill_roots(project_dir: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        roots.push(PathBuf::from(home).join(".config/trogon/skills"));
    }
    roots.push(project_dir.join(".claude/skills"));
    roots.push(project_dir.join(".trogon/skills"));
    roots
}

fn discover_in_root(root: &Path) -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let skill_dir = entry.path();
        if !skill_dir.is_dir() {
            continue;
        }
        let skill_file = skill_dir.join("SKILL.md");
        if !skill_file.is_file() {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&skill_file)
            && let Some(skill) = parse_skill_file(&content, &skill_file, &skill_dir)
        {
            found.push(skill);
        }
    }
    found
}

#[derive(Debug, Default, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
}

/// Parse one `SKILL.md` file: YAML frontmatter + body template.
pub fn parse_skill_file(content: &str, source: &Path, skill_dir: &Path) -> Option<Skill> {
    let dir_name = skill_dir.file_name()?.to_string_lossy().into_owned();
    let (frontmatter, body) = split_frontmatter(content)?;
    let meta: SkillFrontmatter = serde_yaml::from_str(frontmatter).ok()?;
    let name = meta.name.filter(|n| !n.is_empty()).unwrap_or(dir_name);
    if name.is_empty() {
        return None;
    }
    Some(Skill {
        name,
        description: meta.description.unwrap_or_default(),
        body: body.trim().to_string(),
        source: source.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parses_frontmatter_with_name_and_description() {
        let content = "---\nname: custom-name\ndescription: Do things\n---\nRun: $ARGUMENTS\n";
        let dir = Path::new("/tmp/skills/my-skill");
        let skill = parse_skill_file(content, &dir.join("SKILL.md"), dir).unwrap();
        assert_eq!(skill.name, "custom-name");
        assert_eq!(skill.description, "Do things");
        assert_eq!(skill.body, "Run: $ARGUMENTS");
    }

    #[test]
    fn name_falls_back_to_directory() {
        let content = "---\ndescription: From dir\n---\nBody\n";
        let dir = Path::new("/skills/deploy");
        let skill = parse_skill_file(content, &dir.join("SKILL.md"), dir).unwrap();
        assert_eq!(skill.name, "deploy");
        assert_eq!(skill.description, "From dir");
        assert_eq!(skill.body, "Body");
    }

    #[test]
    fn discovery_finds_user_project_and_claude_alias_roots() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let home = tmp.path().join("home");

        fs::create_dir_all(home.join(".config/trogon/skills/global-skill")).unwrap();
        fs::create_dir_all(project.join(".claude/skills/claude-skill")).unwrap();
        fs::create_dir_all(project.join(".trogon/skills/trogon-skill")).unwrap();

        fs::write(
            home.join(".config/trogon/skills/global-skill/SKILL.md"),
            "---\ndescription: Global\n---\nGlobal body\n",
        )
        .unwrap();
        fs::write(
            project.join(".claude/skills/claude-skill/SKILL.md"),
            "---\ndescription: Claude alias\n---\nClaude body\n",
        )
        .unwrap();
        fs::write(
            project.join(".trogon/skills/trogon-skill/SKILL.md"),
            "---\ndescription: Trogon\n---\nTrogon body\n",
        )
        .unwrap();

        let mut by_name = HashMap::new();
        for root in [
            home.join(".config/trogon/skills"),
            project.join(".claude/skills"),
            project.join(".trogon/skills"),
        ] {
            for skill in discover_in_root(&root) {
                by_name.insert(skill.name.clone(), skill);
            }
        }
        let registry = SkillRegistry::new(by_name.into_values());

        assert!(registry.get("global-skill").is_some());
        assert!(registry.get("claude-skill").is_some());
        assert!(registry.get("trogon-skill").is_some());
        assert_eq!(registry.skills().count(), 3);
    }

    #[test]
    fn project_overrides_user_on_name_clash() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".config/trogon/skills/dup")).unwrap();
        fs::create_dir_all(project.join(".trogon/skills/dup")).unwrap();
        fs::write(
            home.join(".config/trogon/skills/dup/SKILL.md"),
            "---\ndescription: user\n---\nuser\n",
        )
        .unwrap();
        fs::write(
            project.join(".trogon/skills/dup/SKILL.md"),
            "---\ndescription: project\n---\nproject\n",
        )
        .unwrap();

        let mut by_name = HashMap::new();
        for root in [
            home.join(".config/trogon/skills"),
            project.join(".trogon/skills"),
        ] {
            for skill in discover_in_root(&root) {
                by_name.insert(skill.name.clone(), skill);
            }
        }
        let registry = SkillRegistry::new(by_name.into_values());
        assert_eq!(registry.get("dup").unwrap().description, "project");
    }

    #[test]
    fn dispatch_expands_template_and_unknown_returns_none() {
        let registry = SkillRegistry::new([Skill {
            name: "test".into(),
            description: String::new(),
            body: "Run: $ARGUMENTS ($1)".into(),
            source: PathBuf::from("/x/SKILL.md"),
        }]);
        assert_eq!(
            registry.dispatch("test", "one two").as_deref(),
            Some("Run: one two (one)")
        );
        assert!(registry.dispatch("nope", "").is_none());
    }

    #[test]
    fn format_skills_list_includes_names_and_descriptions() {
        let registry = SkillRegistry::new([Skill {
            name: "ship".into(),
            description: "Ship it".into(),
            body: String::new(),
            source: PathBuf::from("/x/SKILL.md"),
        }]);
        let list = format_skills_list(&registry);
        assert!(list.contains("ship"));
        assert!(list.contains("Ship it"));
        assert!(list.contains("/skill"));
    }
}
