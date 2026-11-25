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
fn parses_only_the_hyphenated_lowercase_form_of_a_uuid_v7() {
    let v7 = Uuid::now_v7();
    let hyphenated = v7.hyphenated().to_string();
    assert_eq!(ChainId::parse(&hyphenated).unwrap().as_uuid(), v7);

    for spelling in [
        hyphenated.to_uppercase(),
        v7.simple().to_string(),
        v7.braced().to_string(),
        v7.urn().to_string(),
        Uuid::new_v4().hyphenated().to_string(),
        "not-a-uuid".to_string(),
    ] {
        assert_eq!(ChainId::parse(&spelling), Err(ChainIdError::NotUuidV7), "{spelling}");
    }
}

#[test]
fn the_error_message_states_the_rule() {
    assert_eq!(ChainIdError::NotUuidV7.to_string(), "chain id must be a UUIDv7");
}
