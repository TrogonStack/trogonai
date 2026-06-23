use a2a::types::{Artifact, Message, Part};

use crate::error::RedactionError;
use crate::skill_id::SkillId;

pub trait Redactor {
    fn redact_message(&self, message: Message, _skill: &SkillId) -> Result<Message, RedactionError> {
        Ok(message)
    }

    fn redact_artifact(&self, artifact: Artifact, _skill: &SkillId) -> Result<Artifact, RedactionError> {
        Ok(artifact)
    }
}

#[allow(dead_code)]
pub(crate) fn redact_message_parts_with(
    mut message: Message,
    mut transform_part_json: impl FnMut(&[u8]) -> Result<Vec<u8>, RedactionError>,
) -> Result<Message, RedactionError> {
    let mut next_parts = Vec::with_capacity(message.parts.len());
    for part in core::mem::take(&mut message.parts) {
        let wire = serde_json::to_vec(&part)?;
        let out = transform_part_json(&wire)?;
        let parsed: Part = serde_json::from_slice(&out)?;
        next_parts.push(parsed);
    }
    message.parts = next_parts;
    Ok(message)
}

#[allow(dead_code)]
pub(crate) fn redact_artifact_parts_with(
    mut artifact: Artifact,
    mut transform_part_json: impl FnMut(&[u8]) -> Result<Vec<u8>, RedactionError>,
) -> Result<Artifact, RedactionError> {
    let mut next_parts = Vec::with_capacity(artifact.parts.len());
    for part in core::mem::take(&mut artifact.parts) {
        let wire = serde_json::to_vec(&part)?;
        let out = transform_part_json(&wire)?;
        let parsed: Part = serde_json::from_slice(&out)?;
        next_parts.push(parsed);
    }
    artifact.parts = next_parts;
    Ok(artifact)
}

#[cfg(test)]
mod tests;
