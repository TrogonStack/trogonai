use super::*;
use crate::fixtures::SequentialIds;

#[test]
fn generates_a_uuid_v7() {
    let id = ChainId::generate(&SequentialIds::default());
    assert_eq!(id.as_uuid().get_version_num(), 7);
}

#[test]
fn displays_the_hyphenated_form() {
    let id = ChainId::generate(&SequentialIds::default());
    assert_eq!(id.to_string(), id.as_uuid().hyphenated().to_string());
    assert_eq!(id.to_string().len(), 36);
}

#[test]
fn accepts_only_a_uuid_v7() {
    let v7 = Uuid::now_v7();
    assert_eq!(ChainId::from_uuid(v7).unwrap().as_uuid(), v7);
    assert_eq!(ChainId::from_uuid(Uuid::new_v4()), Err(ChainIdError::NotUuidV7));
    assert_eq!(ChainId::from_uuid(Uuid::nil()), Err(ChainIdError::NotUuidV7));
}

#[test]
fn the_error_message_states_the_rule() {
    assert_eq!(ChainIdError::NotUuidV7.to_string(), "chain id must be a UUIDv7");
}
