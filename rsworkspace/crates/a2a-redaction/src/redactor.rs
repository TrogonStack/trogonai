use a2a_types::{Artifact, Message};

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

#[cfg(test)]
mod tests {
    use super::*;
    use a2a_types::Role;

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
}
