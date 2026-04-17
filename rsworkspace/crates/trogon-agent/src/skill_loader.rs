use async_nats::jetstream::{self, kv};

const CONSOLE_SKILLS_BUCKET: &str = "CONSOLE_SKILLS";
const CONSOLE_SKILL_VERSIONS_BUCKET: &str = "CONSOLE_SKILL_VERSIONS";

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

        Ok(Self { skills_kv, versions_kv })
    }

    /// Load content for each skill ID and return a formatted block suitable
    /// for injection into the system prompt. Returns `None` when no skill IDs
    /// are provided or none have content.
    pub async fn load(&self, skill_ids: &[String]) -> Option<String> {
        if skill_ids.is_empty() {
            return None;
        }

        let mut sections = Vec::new();

        for skill_id in skill_ids {
            // Fetch the skill metadata to get the latest_version and display name.
            let (latest_version, skill_name) = match self.skills_kv.get(skill_id).await {
                Ok(Some(bytes)) => {
                    let Ok(meta) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                        continue;
                    };
                    let Some(version) = meta["latest_version"].as_str().map(|s| s.to_string()) else {
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
