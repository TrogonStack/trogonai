use uuid::Uuid;

use super::*;
use crate::fixtures::SequentialIds;

#[test]
fn a_generated_id_is_a_uuidv7_that_parses_back() {
    let id = CheckpointId::generate(&SequentialIds::default());

    assert_eq!(id.as_uuid().get_version_num(), 7);
    assert_eq!(CheckpointId::parse(&id.to_string()), Ok(id));
}

#[test]
fn refuses_another_version_or_spelling() {
    let id = CheckpointId::generate(&SequentialIds::default()).to_string();
    let v4 = "9b2f8f4e-1c3a-4d5e-8f6a-7b8c9d0e1f2a";
    let braced = format!("{{{id}}}");
    let simple = id.replace('-', "");

    for value in [v4, &id.to_uppercase(), &braced, &simple, "not-a-uuid", ""] {
        assert_eq!(CheckpointId::parse(value), Err(CheckpointIdError::NotUuidV7), "{value}");
    }
    assert_eq!(Uuid::parse_str(v4).unwrap().get_version_num(), 4);
}
