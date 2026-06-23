use crate::redactor::Redactor;

pub struct NoopRedactor;

impl Redactor for NoopRedactor {}

#[cfg(test)]
mod tests {
    use super::*;
    use a2a::types::{Artifact, Message, Role};

    use crate::skill_id::SkillId;

    #[test]
    fn message_identity() {
        let r = NoopRedactor;
        let msg = Message {
            message_id: "mid".into(),
            context_id: None,
            task_id: None,
            role: Role::User,
            parts: vec![a2a::types::Part {
                content: a2a::types::PartContent::Text("hello".into()),
                filename: None,
                media_type: None,
                metadata: None,
            }],
            metadata: None,
            extensions: None,
            reference_task_ids: None,
        };
        let out = r
            .redact_message(msg.clone(), &SkillId::new("skill").expect("valid"))
            .unwrap();
        assert_eq!(serde_json::to_value(out).unwrap(), serde_json::to_value(msg).unwrap());
    }

    #[test]
    fn artifact_identity() {
        let r = NoopRedactor;
        let art = Artifact {
            artifact_id: "a".into(),
            name: None,
            description: None,
            parts: vec![],
            metadata: None,
            extensions: None,
        };
        let out = r
            .redact_artifact(art.clone(), &SkillId::new("skill").expect("valid"))
            .unwrap();
        assert_eq!(serde_json::to_value(out).unwrap(), serde_json::to_value(art).unwrap());
    }
}
