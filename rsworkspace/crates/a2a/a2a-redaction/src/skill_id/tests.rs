use super::*;

#[test]
fn serde_roundtrip() {
    let id = SkillId::new("skill-a").expect("valid");
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, "\"skill-a\"");
    let parsed: SkillId = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, id);
}

#[test]
fn display_matches_inner() {
    let id = SkillId::new("x").expect("valid");
    assert_eq!(format!("{id}"), "x");
}

#[test]
fn empty_is_rejected() {
    assert_eq!(SkillId::new("").unwrap_err(), SkillIdError::Empty);
}

#[test]
fn path_separator_is_rejected() {
    assert!(matches!(
        SkillId::new("a/b").unwrap_err(),
        SkillIdError::ForbiddenCharacter('/')
    ));
    assert!(matches!(
        SkillId::new("a\\b").unwrap_err(),
        SkillIdError::ForbiddenCharacter('\\')
    ));
}

#[test]
fn parent_dir_traversal_is_rejected() {
    assert_eq!(SkillId::new("..").unwrap_err(), SkillIdError::PathTraversal);
    assert_eq!(SkillId::new("a..b").unwrap_err(), SkillIdError::PathTraversal);
}

#[test]
fn deserialize_runs_validator() {
    let bad = serde_json::from_str::<SkillId>("\"a/b\"");
    assert!(bad.is_err(), "deserialize must reject forbidden chars, got: {bad:?}");
}
