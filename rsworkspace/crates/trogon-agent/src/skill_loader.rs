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
/// into the agent's system prompt.
#[derive(Clone)]
pub struct SkillLoader {
    skills_kv: kv::Store,
    versions_kv: kv::Store,
}

impl SkillLoader {
    /// Open (or create) the console skill KV buckets.
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
                sections.push(format!("## Skill: {skill_name}\n\n{content}"));
            }
        }

        if sections.is_empty() {
            return None;
        }

        Some(format!(
            "# Available Skills\n\nThe following skills define specialized knowledge and procedures you must follow:\n\n{}",
            sections.join("\n\n---\n\n")
        ))
    }
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

    impl Default for MockSkillLoader {
        fn default() -> Self {
            Self::new()
        }
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
            let mut sections = Vec::new();
            for id in skill_ids {
                if let Some(c) = self.content.get(id) {
                    sections.push(format!("## Skill: {id}\n\n{c}"));
                }
            }
            let result = if sections.is_empty() {
                None
            } else {
                Some(format!(
                    "# Available Skills\n\nThe following skills define specialized knowledge and procedures you must follow:\n\n{}",
                    sections.join("\n\n---\n\n")
                ))
            };
            Box::pin(std::future::ready(result))
        }
    }
}
