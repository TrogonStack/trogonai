use std::future::Future;
use std::pin::Pin;

use async_nats::jetstream::{self, kv};

const CONSOLE_SKILLS_BUCKET: &str = "CONSOLE_SKILLS";
const CONSOLE_SKILL_VERSIONS_BUCKET: &str = "CONSOLE_SKILL_VERSIONS";

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait SkillLoading: Send + Sync + 'static {
    fn load<'a>(&'a self, skill_ids: &'a [String]) -> BoxFuture<'a, Option<String>>;
}

#[derive(Clone)]
pub struct SkillLoader {
    skills_kv: kv::Store,
    versions_kv: kv::Store,
}

impl SkillLoader {
    pub async fn open(js: &jetstream::Context) -> Result<Self, String> {
        let skills_kv = js
            .create_or_update_key_value(kv::Config {
                bucket: CONSOLE_SKILLS_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;

        let versions_kv = js
            .create_or_update_key_value(kv::Config {
                bucket: CONSOLE_SKILL_VERSIONS_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;

        Ok(Self {
            skills_kv,
            versions_kv,
        })
    }

    async fn load_impl(&self, skill_ids: &[String]) -> Option<String> {
        if skill_ids.is_empty() {
            return None;
        }

        let mut sections = Vec::new();

        for skill_id in skill_ids {
            let (latest_version, skill_name) = match self.skills_kv.get(skill_id).await {
                Ok(Some(bytes)) => {
                    let Ok(meta) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                        continue;
                    };
                    let Some(version) = meta["latest_version"].as_str().map(|s| s.to_string())
                    else {
                        continue;
                    };
                    let name = meta["name"]
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| skill_id.clone());
                    (version, name)
                }
                _ => continue,
            };

            let version_key = format!("{skill_id}.{latest_version}");
            let content = match self.versions_kv.get(&version_key).await {
                Ok(Some(bytes)) => {
                    let Ok(ver) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                        continue;
                    };
                    match ver["content"].as_str().map(|s| s.to_string()) {
                        Some(c) => c,
                        None => continue,
                    }
                }
                _ => continue,
            };

            if !content.is_empty() {
                sections.push((skill_name, content));
            }
        }

        format_skill_sections(sections)
    }
}

pub(crate) fn format_skill_sections(sections: Vec<(String, String)>) -> Option<String> {
    if sections.is_empty() {
        return None;
    }
    let body = sections
        .into_iter()
        .map(|(name, content)| format!("## Skill: {name}\n\n{content}"))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");
    Some(format!(
        "# Available Skills\n\nThe following skills define specialized knowledge and procedures you must follow:\n\n{body}"
    ))
}

impl SkillLoading for SkillLoader {
    fn load<'a>(&'a self, skill_ids: &'a [String]) -> BoxFuture<'a, Option<String>> {
        Box::pin(self.load_impl(skill_ids))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sec(name: &str, content: &str) -> (String, String) {
        (name.to_string(), content.to_string())
    }

    #[test]
    fn empty_sections_returns_none() {
        assert!(format_skill_sections(vec![]).is_none());
    }

    #[test]
    fn single_skill_has_correct_structure() {
        let out = format_skill_sections(vec![sec("Coding", "Write tests.")]).unwrap();
        assert!(out.starts_with("# Available Skills\n\n"));
        assert!(out.contains("## Skill: Coding\n\nWrite tests."));
    }

    #[test]
    fn multiple_skills_joined_with_separator() {
        let out = format_skill_sections(vec![
            sec("Alpha", "Do alpha."),
            sec("Beta", "Do beta."),
        ])
        .unwrap();
        assert!(out.contains("## Skill: Alpha\n\nDo alpha."));
        assert!(out.contains("## Skill: Beta\n\nDo beta."));
        assert!(out.contains("\n\n---\n\n"), "sections must be separated by ---");
    }

    #[test]
    fn order_is_preserved() {
        let out = format_skill_sections(vec![
            sec("First", "1"),
            sec("Second", "2"),
            sec("Third", "3"),
        ])
        .unwrap();
        let pos_first = out.find("## Skill: First").unwrap();
        let pos_second = out.find("## Skill: Second").unwrap();
        let pos_third = out.find("## Skill: Third").unwrap();
        assert!(pos_first < pos_second);
        assert!(pos_second < pos_third);
    }

    #[test]
    fn skill_name_with_special_chars() {
        let out = format_skill_sections(vec![sec("C++ & Rust", "```rust\nfn f() {}\n```")]).unwrap();
        assert!(out.contains("## Skill: C++ & Rust"));
        assert!(out.contains("```rust"));
    }

    #[test]
    fn single_skill_has_no_separator() {
        let out = format_skill_sections(vec![sec("Solo", "content")]).unwrap();
        assert!(!out.contains("---"), "single skill must not contain separator");
    }

    #[test]
    fn header_preamble_line_is_present() {
        let out = format_skill_sections(vec![sec("A", "x")]).unwrap();
        assert!(
            out.contains("The following skills define specialized knowledge"),
            "output must contain the preamble line: {out:?}"
        );
    }

    #[test]
    fn multiline_skill_content_is_preserved() {
        let content = "line one\nline two\nline three";
        let out = format_skill_sections(vec![sec("Docs", content)]).unwrap();
        assert!(out.contains("line one\nline two\nline three"));
    }

    #[test]
    fn separator_is_only_between_skills_not_at_end() {
        let out = format_skill_sections(vec![
            sec("First", "a"),
            sec("Second", "b"),
        ])
        .unwrap();
        // Exactly one separator between the two skills.
        let sep_count = out.matches("\n\n---\n\n").count();
        assert_eq!(sep_count, 1, "must have exactly one separator: {out:?}");
        // No separator after the last skill.
        assert!(!out.ends_with("---\n\n"), "must not have trailing separator");
    }

    #[test]
    fn skill_section_header_format_is_h2() {
        let out = format_skill_sections(vec![sec("MySkill", "body")]).unwrap();
        assert!(out.contains("## Skill: MySkill\n\nbody"));
    }
}
