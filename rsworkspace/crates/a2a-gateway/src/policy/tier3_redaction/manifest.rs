use std::collections::BTreeMap;

use a2a_redaction::{SkillId, WasmBundlePath};
use tracing::warn;

use super::json_path::to_json_pointer;
use super::rewrite::RewriteKind;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tier3SkillManifest {
    skill_id: SkillId,
    json_path: String,
    kind: RewriteKind,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Tier3SkillManifestError {
    #[error("json_path must not be empty")]
    EmptyJsonPath,
    #[error("json_path {0:?} is not a valid JSONPath or JSON Pointer (expected `$.`, `$`, or `/`-prefixed)")]
    InvalidJsonPath(String),
}

impl Tier3SkillManifest {
    /// Validated constructor: rejects empty json_path values + paths
    /// that aren't a recognized shape, so an invalid manifest can't
    /// reach the evaluator and silently no-op.
    pub fn new(
        skill_id: SkillId,
        json_path: impl Into<String>,
        kind: RewriteKind,
    ) -> Result<Self, Tier3SkillManifestError> {
        let json_path = json_path.into();
        if json_path.trim().is_empty() {
            return Err(Tier3SkillManifestError::EmptyJsonPath);
        }
        if !is_recognized_json_path(&json_path) {
            return Err(Tier3SkillManifestError::InvalidJsonPath(json_path));
        }
        Ok(Self {
            skill_id,
            json_path,
            kind,
        })
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
        // The wasm module is registered under the bundle's file stem,
        // so we ALWAYS pin the typed `skill_id` to the file stem. A
        // manifest `skill_id` field that disagrees with the stem is
        // logged as a configuration warning but does not override the
        // identity used to look up the wasm module — that drift would
        // otherwise produce a silent passthrough-allow because the
        // host would find no module at the manifest's overridden id.
        if let Some(manifest_override) = raw.get("skill_id").and_then(|v| v.as_str())
            && manifest_override != skill_id.as_str()
        {
            warn!(
                file_skill = %skill_id,
                manifest_skill = %manifest_override,
                "tier-3 manifest skill_id disagrees with bundle file stem; using file stem so the wasm module lookup hits"
            );
        }
        Self::new(skill_id, json_path, kind).ok()
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

/// Accept only the JSON-path shapes the redactor can actually
/// resolve at runtime. Delegating to `to_json_pointer` ties manifest
/// validation to the same parser the gate uses, so a value that
/// passes here is guaranteed to round-trip to a JSON Pointer at
/// evaluation time. A loose prefix check (`$`, `$.…`, `/…`) lets
/// patterns like `$..` or `$.[]` through, and the gate would then
/// silently skip the skill with a warning at runtime — surfacing the
/// error at config-load time fails closed instead.
fn is_recognized_json_path(path: &str) -> bool {
    to_json_pointer(path).is_some()
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
