use super::*;
use a2a::types::{PartContent, Role};

struct DefaultRedactorByTrait;

impl Redactor for DefaultRedactorByTrait {}

#[test]
fn default_redact_message_identity() {
    let r = DefaultRedactorByTrait;
    let msg = Message {
        message_id: "m".into(),
        context_id: None,
        task_id: None,
        role: Role::User,
        parts: vec![],
        metadata: None,
        extensions: None,
        reference_task_ids: None,
    };
    assert_eq!(
        serde_json::to_value(
            r.redact_message(msg.clone(), &SkillId::new("s").expect("valid"))
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&msg).unwrap()
    );
}

#[test]
fn part_loop_identity_preserves_shapes() {
    let msg_in = Message {
        message_id: "m".into(),
        context_id: None,
        task_id: None,
        role: Role::Agent,
        parts: vec![a2a::types::Part {
            content: PartContent::Text("secret".into()),
            filename: None,
            media_type: None,
            metadata: None,
        }],
        metadata: None,
        extensions: None,
        reference_task_ids: None,
    };
    let msg_out = redact_message_parts_with(msg_in.clone(), |b| Ok(b.to_vec())).expect("identity json transform");
    assert_eq!(
        serde_json::to_value(&msg_out).unwrap(),
        serde_json::to_value(&msg_in).unwrap()
    );
}
