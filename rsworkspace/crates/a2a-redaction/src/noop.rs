use crate::redactor::Redactor;

pub struct NoopRedactor;

impl Redactor for NoopRedactor {}

#[cfg(test)]
mod tests {
    use super::*;
    use a2a_types::{Artifact, Message, Role};

    use crate::skill_id::SkillId;

    #[test]
    fn message_identity() {
        let r = NoopRedactor;
        let msg = Message {
            message_id: "mid".into(),
            role: Role::User.into(),
            parts: vec![a2a_types::Part {
                content: Some(a2a_types::part::Content::Text("hello".into())),
                ..Default::default()
            }],
            ..Default::default()
        };
        let out = r.redact_message(msg.clone(), &SkillId::new("skill")).unwrap();
        assert_eq!(
            serde_json::to_value(out).unwrap(),
            serde_json::to_value(msg).unwrap()
        );
    }

    #[test]
    fn artifact_identity() {
        let r = NoopRedactor;
        let art = Artifact {
            artifact_id: "a".into(),
            ..Default::default()
        };
        let out = r.redact_artifact(art.clone(), &SkillId::new("skill")).unwrap();
        assert_eq!(
            serde_json::to_value(out).unwrap(),
            serde_json::to_value(art).unwrap()
        );
    }
}
