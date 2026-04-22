use async_nats::jetstream::{self, kv};
use bytes::Bytes;
use futures_util::StreamExt as _;

use crate::models::skill::{Skill, SkillVersion};
use crate::store::traits::SkillRepository;

pub const SKILLS_BUCKET: &str = "CONSOLE_SKILLS";
pub const SKILL_VERSIONS_BUCKET: &str = "CONSOLE_SKILL_VERSIONS";

#[derive(Clone)]
pub struct SkillStore {
    kv: kv::Store,
    versions_kv: kv::Store,
}

impl SkillStore {
    pub async fn open(js: &jetstream::Context) -> Result<Self, String> {
        let kv = js
            .create_or_update_key_value(kv::Config {
                bucket: SKILLS_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;

        let versions_kv = js
            .create_or_update_key_value(kv::Config {
                bucket: SKILL_VERSIONS_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;

        Ok(Self { kv, versions_kv })
    }

    pub async fn list(&self) -> Result<Vec<Skill>, String> {
        let mut keys = self.kv.keys().await.map_err(|e| e.to_string())?;
        let mut skills = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(|e| e.to_string())?;
            if let Some(skill) = self.get(&key).await? {
                skills.push(skill);
            }
        }
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(skills)
    }

    pub async fn get(&self, id: &str) -> Result<Option<Skill>, String> {
        match self.kv.get(id).await.map_err(|e| e.to_string())? {
            None => Ok(None),
            Some(bytes) => serde_json::from_slice::<Skill>(&bytes)
                .map(Some)
                .map_err(|e| e.to_string()),
        }
    }

    pub async fn put(&self, skill: &Skill) -> Result<(), String> {
        let bytes = serde_json::to_vec(skill).expect("Skill serialization");
        self.kv
            .put(&skill.id, Bytes::from(bytes))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<(), String> {
        self.kv.delete(id).await.map_err(|e| e.to_string())
    }

    // Versions use key format: "{skill_id}.{version}" e.g. "pdf.20260417"
    pub async fn list_versions(&self, skill_id: &str) -> Result<Vec<SkillVersion>, String> {
        let prefix = format!("{skill_id}.");
        let mut keys = self.versions_kv.keys().await.map_err(|e| e.to_string())?;
        let mut versions = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(|e| e.to_string())?;
            if !key.starts_with(&prefix) {
                continue;
            }
            if let Some(bytes) = self
                .versions_kv
                .get(&key)
                .await
                .map_err(|e| e.to_string())?
                && let Ok(v) = serde_json::from_slice::<SkillVersion>(&bytes)
            {
                versions.push(v);
            }
        }
        versions.sort_by(|a, b| b.version.cmp(&a.version));
        Ok(versions)
    }

    pub async fn put_version(&self, version: &SkillVersion) -> Result<(), String> {
        let key = format!("{}.{}", version.skill_id, version.version);
        let bytes = serde_json::to_vec(version).expect("SkillVersion serialization");
        self.versions_kv
            .put(&key, Bytes::from(bytes))
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

impl SkillRepository for SkillStore {
    fn list(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<Skill>, String>> + Send + '_>>
    {
        Box::pin(async move { self.list().await })
    }
    fn get<'a>(
        &'a self,
        id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<Skill>, String>> + Send + 'a>,
    > {
        Box::pin(async move { self.get(id).await })
    }
    fn put<'a>(
        &'a self,
        skill: &'a Skill,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move { self.put(skill).await })
    }
    fn delete<'a>(
        &'a self,
        id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move { self.delete(id).await })
    }
    fn list_versions<'a>(
        &'a self,
        skill_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<SkillVersion>, String>> + Send + 'a>,
    > {
        Box::pin(async move { self.list_versions(skill_id).await })
    }
    fn put_version<'a>(
        &'a self,
        version: &'a SkillVersion,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move { self.put_version(version).await })
    }
}
