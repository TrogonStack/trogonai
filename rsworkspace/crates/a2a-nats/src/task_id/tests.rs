use super::*;

#[test]
fn a2a_task_id_valid() {
    assert!(A2aTaskId::new("task-1").is_ok());
    assert_eq!(A2aTaskId::new("t").unwrap().as_str(), "t");
}

#[test]
fn a2a_task_id_rejects_invalid() {
    assert!(A2aTaskId::new("").is_err());
    assert!(A2aTaskId::new("a.b").is_err());
    assert!(A2aTaskId::new("a*").is_err());
    assert!(A2aTaskId::new("a>").is_err());
}

#[test]
fn a2a_task_id_generate_unique() {
    let a = A2aTaskId::generate();
    let b = A2aTaskId::generate();
    assert_ne!(a.as_str(), b.as_str());
    assert_eq!(a.as_str().len(), 32, "UUID v7 simple form is 32 hex chars");
}

#[test]
fn a2a_task_id_display() {
    let id = A2aTaskId::new("abc").unwrap();
    assert_eq!(format!("{id}"), "abc");
}

#[test]
fn a2a_task_id_derefs_to_str() {
    let id = A2aTaskId::new("task-deref").unwrap();
    let s: &str = &id;
    assert_eq!(s, "task-deref");
}

#[test]
fn task_id_error_display() {
    assert_eq!(TaskIdError::Empty.to_string(), "task_id must not be empty");
    assert_eq!(
        TaskIdError::InvalidCharacter('.').to_string(),
        "task_id contains invalid character: '.'"
    );
    assert_eq!(
        TaskIdError::TooLong(129).to_string(),
        "task_id is too long: 129 characters (max 128)"
    );
}

#[test]
fn a2a_task_id_rejects_too_long() {
    let long = "a".repeat(129);
    let err = A2aTaskId::new(&long).unwrap_err();
    assert!(matches!(err, TaskIdError::TooLong(129)));
}
