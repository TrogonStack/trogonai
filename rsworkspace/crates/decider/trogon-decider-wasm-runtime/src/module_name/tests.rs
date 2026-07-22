use super::*;

#[test]
fn accepts_non_empty_module_name() {
    let module_name = ModuleName::new("scheduler.schedules").unwrap();
    assert_eq!(module_name.as_str(), "scheduler.schedules");
    assert_eq!(module_name.to_string(), "scheduler.schedules");
}

#[test]
fn rejects_empty_module_name() {
    assert_eq!(ModuleName::new("").unwrap_err(), ModuleNameError::Empty);
}

#[test]
fn rejects_control_characters() {
    assert_eq!(
        ModuleName::new("bad\tname").unwrap_err(),
        ModuleNameError::ContainsControlCharacter
    );
}

#[test]
fn from_str_and_try_from_agree() {
    let expected = ModuleName::new("a").unwrap();
    assert_eq!("a".parse::<ModuleName>().unwrap(), expected);
    assert_eq!(ModuleName::try_from("a").unwrap(), expected);
    assert_eq!(ModuleName::try_from("a".to_string()).unwrap(), expected);
}

#[test]
fn borrows_and_derefs_as_str() {
    let module_name = ModuleName::new("a").unwrap();
    let borrowed: &str = module_name.borrow();
    assert_eq!(borrowed, "a");
    assert_eq!(module_name.as_ref(), "a");
}

#[test]
fn rejects_snapshot_id_delimiters() {
    assert_eq!(
        ModuleName::new("a@b").expect_err("'@' must be rejected"),
        ModuleNameError::ContainsReservedCharacter
    );
    assert_eq!(
        ModuleName::new("a/b").expect_err("'/' must be rejected"),
        ModuleNameError::ContainsReservedCharacter
    );
}
