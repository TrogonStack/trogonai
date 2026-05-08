use std::future::Future;
use std::pin::Pin;

use async_nats::jetstream::{self, kv};

const CONSOLE_SKILLS_BUCKET: &str = "CONSOLE_SKILLS";
const CONSOLE_SKILL_VERSIONS_BUCKET: &str = "CONSOLE_SKILL_VERSIONS";

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ── Trait ─────────────────────────────────────────────────────────────────────

pub trait SkillLoading: Send + Sync + 'static {
    fn load<'a>(&'a self, skill_ids: &'a [String]) -> BoxFuture<'a, Option<String>>;
}

// ── Real implementation ───────────────────────────────────────────────────────

/// Reads skill content from the console KV store and formats it for injection
/// into the system prompt.
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

/// Format collected `(name, content)` pairs into the system-prompt block
/// injected before every conversation. Returns `None` when the list is empty.
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

// ── Mock (test only) ──────────────────────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::SkillLoading;
    use std::collections::HashMap;

    pub struct MockSkillLoader {
        content: HashMap<String, String>,
    }

    impl MockSkillLoader {
        pub fn new() -> Self {
            Self {
                content: HashMap::new(),
            }
        }

        pub fn insert(&mut self, skill_id: &str, content: &str) {
            self.content
                .insert(skill_id.to_string(), content.to_string());
        }
    }

    impl SkillLoading for MockSkillLoader {
        fn load<'a>(
            &'a self,
            skill_ids: &'a [String],
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>>
        {
            let sections = skill_ids
                .iter()
                .filter_map(|id| self.content.get(id).map(|c| (id.clone(), c.clone())))
                .collect();
            let result = super::format_skill_sections(sections);
            Box::pin(std::future::ready(result))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_empty_sections_returns_none() {
        assert!(format_skill_sections(vec![]).is_none());
    }

    #[test]
    fn format_single_skill() {
        let result = format_skill_sections(vec![("Coding".into(), "Write clean code.".into())]);
        let text = result.unwrap();
        assert!(text.starts_with("# Available Skills\n\n"));
        assert!(text.contains("## Skill: Coding\n\nWrite clean code."));
    }

    #[test]
    fn format_multiple_skills_joined_by_separator() {
        let result = format_skill_sections(vec![
            ("Alpha".into(), "Do alpha things.".into()),
            ("Beta".into(), "Do beta things.".into()),
        ]);
        let text = result.unwrap();
        assert!(text.contains("## Skill: Alpha"));
        assert!(text.contains("## Skill: Beta"));
        assert!(text.contains("\n\n---\n\n"));
    }

    #[test]
    fn mock_loader_returns_none_for_empty_skill_ids() {
        use mock::MockSkillLoader;
        use super::SkillLoading;
        let loader = MockSkillLoader::new();
        let result = futures::executor::block_on(loader.load(&[]));
        assert!(result.is_none());
    }

    #[test]
    fn mock_loader_returns_none_for_unknown_ids() {
        use mock::MockSkillLoader;
        use super::SkillLoading;
        let loader = MockSkillLoader::new();
        let ids = vec!["unknown".to_string()];
        let result = futures::executor::block_on(loader.load(&ids));
        assert!(result.is_none());
    }

    #[test]
    fn mock_loader_returns_formatted_skills() {
        use mock::MockSkillLoader;
        use super::SkillLoading;
        let mut loader = MockSkillLoader::new();
        loader.insert("sk1", "Be concise.");
        loader.insert("sk2", "Use examples.");
        let ids = vec!["sk1".to_string(), "sk2".to_string()];
        let text = futures::executor::block_on(loader.load(&ids)).unwrap();
        assert!(text.contains("## Skill: sk1\n\nBe concise."));
        assert!(text.contains("## Skill: sk2\n\nUse examples."));
    }

    #[test]
    fn mock_loader_preserves_skill_id_order() {
        use mock::MockSkillLoader;
        use super::SkillLoading;
        let mut loader = MockSkillLoader::new();
        loader.insert("first", "content-1");
        loader.insert("second", "content-2");
        let ids = vec!["first".to_string(), "second".to_string()];
        let text = futures::executor::block_on(loader.load(&ids)).unwrap();
        let pos_first = text.find("content-1").unwrap();
        let pos_second = text.find("content-2").unwrap();
        assert!(pos_first < pos_second);
    }

    #[test]
    fn format_skill_sections_with_empty_content_includes_section() {
        // format_skill_sections is a pure formatter with no empty-content filter;
        // load_impl is responsible for filtering before calling this function.
        let result = format_skill_sections(vec![("Guide".into(), "".into())]);
        let text = result.unwrap();
        assert!(
            text.contains("## Skill: Guide"),
            "section header must appear even when content is empty; got: {text:?}"
        );
    }
}
