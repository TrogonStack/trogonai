use super::*;

#[test]
fn accepts_non_empty_module_version() {
    let module_version = ModuleVersion::new("0.1.0").unwrap();
    assert_eq!(module_version.as_str(), "0.1.0");
    assert_eq!(module_version.to_string(), "0.1.0");
}

#[test]
fn rejects_empty_module_version() {
    assert_eq!(ModuleVersion::new("").unwrap_err(), ModuleVersionError::Empty);
}

#[test]
fn rejects_control_characters() {
    assert_eq!(
        ModuleVersion::new("0.1\n0").unwrap_err(),
        ModuleVersionError::ContainsControlCharacter
    );
}

#[test]
fn from_str_and_try_from_agree() {
    let expected = ModuleVersion::new("a").unwrap();
    assert_eq!("a".parse::<ModuleVersion>().unwrap(), expected);
    assert_eq!(ModuleVersion::try_from("a").unwrap(), expected);
    assert_eq!(ModuleVersion::try_from("a".to_string()).unwrap(), expected);
}

#[test]
fn borrows_and_derefs_as_str() {
    let module_version = ModuleVersion::new("a").unwrap();
    let borrowed: &str = module_version.borrow();
    assert_eq!(borrowed, "a");
    assert_eq!(module_version.as_ref(), "a");
}

#[test]
fn rejects_snapshot_id_delimiters() {
    assert_eq!(
        ModuleVersion::new("1@2").expect_err("'@' must be rejected"),
        ModuleVersionError::ContainsReservedCharacter
    );
    assert_eq!(
        ModuleVersion::new("1/2").expect_err("'/' must be rejected"),
        ModuleVersionError::ContainsReservedCharacter
    );
}
