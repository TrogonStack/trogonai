use std::collections::BTreeMap;

use a2a_redaction::{SkillId, WasmBundlePath};
use tracing::warn;

use super::rewrite::RewriteKind;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tier3SkillManifest {
    skill_id: SkillId,
    json_path: String,
    kind: RewriteKind,
}

impl Tier3SkillManifest {
    pub fn new(skill_id: SkillId, json_path: impl Into<String>, kind: RewriteKind) -> Self {
        Self {
            skill_id,
            json_path: json_path.into(),
            kind,
        }
    }

    pub fn parse(skill_id: SkillId, raw: &serde_json::Value) -> Option<Self> {
        let json_path = raw.get("json_path")?.as_str()?.to_owned();
        if json_path.is_empty() {
            return None;
        }
        let kind = raw
            .get("kind")
            .and_then(|v| v.as_str())
            .and_then(RewriteKind::from_manifest_str)
            .unwrap_or(RewriteKind::Replaced);
        let manifest_skill = raw
            .get("skill_id")
            .and_then(|v| v.as_str())
            .map(SkillId::new)
            .unwrap_or_else(|| skill_id.clone());
        if manifest_skill != skill_id {
            warn!(
                file_skill = %skill_id,
                manifest_skill = %manifest_skill,
                "tier-3 manifest skill_id mismatch; using manifest value"
            );
        }
        Some(Self {
            skill_id: manifest_skill,
            json_path,
            kind,
        })
    }

    pub fn skill_id(&self) -> &SkillId {
        &self.skill_id
    }

    pub fn json_path(&self) -> &str {
        &self.json_path
    }

    pub fn kind(&self) -> &RewriteKind {
        &self.kind
    }
}

pub fn load_tier3_manifests_from_bundle(
    bundle_path: &WasmBundlePath,
    skill_slugs: &[SkillId],
) -> BTreeMap<SkillId, Tier3SkillManifest> {
    let mut manifests = BTreeMap::new();
    for skill in skill_slugs {
        let path = bundle_path.join_skill_manifest(skill);
        match std::fs::read_to_string(&path) {
            Err(err) => {
                warn!(
                    skill = %skill,
                    path = %path.display(),
                    error = %err,
                    "tier-3 manifest missing or unreadable; skill skipped at redaction site"
                );
            }
            Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                Err(err) => {
                    warn!(
                        skill = %skill,
                        path = %path.display(),
                        error = %err,
                        "tier-3 manifest JSON invalid; skill skipped"
                    );
                }
                Ok(value) => {
                    if let Some(manifest) = Tier3SkillManifest::parse(skill.clone(), &value) {
                        manifests.insert(manifest.skill_id().clone(), manifest);
                    } else {
                        warn!(
                            skill = %skill,
                            path = %path.display(),
                            "tier-3 manifest missing json_path; skill skipped"
                        );
                    }
                }
            },
        }
    }
    manifests
}
