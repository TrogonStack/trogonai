use super::*;

#[test]
fn accepts_non_empty_command_type() {
    let command_type = CommandType::new("scheduler.schedules.v1.CreateSchedule").unwrap();
    assert_eq!(command_type.as_str(), "scheduler.schedules.v1.CreateSchedule");
    assert_eq!(command_type.to_string(), "scheduler.schedules.v1.CreateSchedule");
}

#[test]
fn rejects_empty_command_type() {
    assert_eq!(CommandType::new("").unwrap_err(), CommandTypeError::Empty);
}

#[test]
fn rejects_control_characters() {
    assert_eq!(
        CommandType::new("bad\ntype").unwrap_err(),
        CommandTypeError::ContainsControlCharacter
    );
}

#[test]
fn from_str_and_try_from_agree() {
    let expected = CommandType::new("a").unwrap();
    assert_eq!("a".parse::<CommandType>().unwrap(), expected);
    assert_eq!(CommandType::try_from("a").unwrap(), expected);
    assert_eq!(CommandType::try_from("a".to_string()).unwrap(), expected);
}

#[test]
fn borrows_and_derefs_as_str() {
    let command_type = CommandType::new("a").unwrap();
    let borrowed: &str = command_type.borrow();
    assert_eq!(borrowed, "a");
    assert_eq!(command_type.as_ref(), "a");
}
