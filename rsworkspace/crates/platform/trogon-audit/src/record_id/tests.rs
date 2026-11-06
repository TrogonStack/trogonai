use super::*;
use crate::fixtures::SequentialIds;

#[test]
fn generates_a_uuid_v7() {
    let id = RecordId::generate(&SequentialIds::default());
    assert_eq!(id.as_uuid().get_version_num(), 7);
}

#[test]
fn displays_the_hyphenated_form() {
    let id = RecordId::generate(&SequentialIds::default());
    assert_eq!(id.to_string(), id.as_uuid().hyphenated().to_string());
    assert_eq!(id.to_string().len(), 36);
}

#[test]
fn accepts_only_a_uuid_v7() {
    let v7 = Uuid::now_v7();
    assert_eq!(RecordId::from_uuid(v7).unwrap().as_uuid(), v7);
    assert_eq!(RecordId::from_uuid(Uuid::new_v4()), Err(RecordIdError::NotUuidV7));
}

#[test]
fn the_error_message_states_the_rule() {
    assert_eq!(RecordIdError::NotUuidV7.to_string(), "record id must be a UUIDv7");
}
