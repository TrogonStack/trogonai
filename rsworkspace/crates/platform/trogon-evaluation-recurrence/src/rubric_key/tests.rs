use trogonai_proto::evaluation::rubrics::v1alpha1::{RubricId, RubricVersionRef};

use super::{RubricKey, RubricKeyError};

#[test]
fn accepts_a_rubric_id_and_version() {
    assert!(RubricKey::new("rubric-1", "v1").is_ok());
}

#[test]
fn rejects_an_empty_rubric_id() {
    assert_eq!(RubricKey::new("", "v1"), Err(RubricKeyError::MissingRubricId));
}

#[test]
fn rejects_an_empty_version() {
    assert_eq!(RubricKey::new("rubric-1", ""), Err(RubricKeyError::MissingVersion));
}

#[test]
fn converts_a_complete_reference() {
    let reference = RubricVersionRef {
        rubric_id: RubricId {
            value: "rubric-1".to_string(),
        }
        .into(),
        version: "v1".to_string(),
    };

    assert_eq!(RubricKey::try_from(&reference), RubricKey::new("rubric-1", "v1"));
}

#[test]
fn rejects_a_reference_without_a_rubric_id() {
    let reference = RubricVersionRef {
        version: "v1".to_string(),
        ..Default::default()
    };

    assert_eq!(RubricKey::try_from(&reference), Err(RubricKeyError::MissingRubricId));
}
