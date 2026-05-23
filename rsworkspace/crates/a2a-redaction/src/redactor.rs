use a2a_types::{Artifact, Message, Part};

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
mod tests {
    use super::*;
    use a2a_types::{part, Role};

    struct DefaultRedactorByTrait;

    impl Redactor for DefaultRedactorByTrait {}

    #[test]
    fn default_redact_message_identity() {
        let r = DefaultRedactorByTrait;
        let msg = Message {
            message_id: "m".into(),
            role: Role::User.into(),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(r.redact_message(msg.clone(), &SkillId::new("s")).unwrap()).unwrap(),
            serde_json::to_value(&msg).unwrap()
        );
    }

    #[test]
    fn part_loop_identity_preserves_shapes() {
        let msg_in = Message {
            message_id: "m".into(),
            role: Role::Agent.into(),
            parts: vec![a2a_types::Part {
                content: Some(part::Content::Text("secret".into())),
                ..Default::default()
            }],
            ..Default::default()
        };
        let msg_out =
            redact_message_parts_with(msg_in.clone(), |b| Ok(b.to_vec())).expect("identity json transform");
        assert_eq!(
            serde_json::to_value(&msg_out).unwrap(),
            serde_json::to_value(&msg_in).unwrap()
        );
    }
}
