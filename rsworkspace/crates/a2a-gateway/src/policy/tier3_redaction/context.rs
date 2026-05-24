use std::collections::BTreeMap;

use a2a_redaction::SkillId;

use super::manifest::Tier3SkillManifest;

#[derive(Clone, Debug)]
pub struct Tier3EvaluationContext {
    method: String,
    caller_id: Option<String>,
    payload: serde_json::Value,
    skill_manifests: BTreeMap<SkillId, Tier3SkillManifest>,
}

impl Tier3EvaluationContext {
    pub fn new(
        method: impl Into<String>,
        caller_id: Option<String>,
        payload: serde_json::Value,
        skill_manifests: BTreeMap<SkillId, Tier3SkillManifest>,
    ) -> Self {
        Self {
            method: method.into(),
            caller_id,
            payload,
            skill_manifests,
        }
    }

    pub fn from_json_rpc_payload(
        method: impl Into<String>,
        caller_id: Option<String>,
        raw_payload: &[u8],
        skill_manifests: BTreeMap<SkillId, Tier3SkillManifest>,
    ) -> Self {
        let payload = serde_json::from_slice(raw_payload).unwrap_or(serde_json::Value::Null);
        Self::new(method, caller_id, payload, skill_manifests)
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn caller_id(&self) -> Option<&str> {
        self.caller_id.as_deref()
    }

    pub fn payload(&self) -> &serde_json::Value {
        &self.payload
    }

    pub fn payload_mut(&mut self) -> &mut serde_json::Value {
        &mut self.payload
    }

    pub fn skill_manifests(&self) -> &BTreeMap<SkillId, Tier3SkillManifest> {
        &self.skill_manifests
    }

    pub fn into_payload_bytes(self) -> Result<bytes::Bytes, serde_json::Error> {
        serde_json::to_vec(&self.payload).map(bytes::Bytes::from)
    }
}
