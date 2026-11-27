use super::{CommandPolicy, CommandVetoError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn s(items: &[&str]) -> Vec<String> {
    items.iter().map(|i| (*i).to_string()).collect()
}

#[test]
fn command_deny_and_allow() -> TestResult {
    let policy = CommandPolicy::new(&s(&[r"^rm\s", r"curl .*\| *sh"]), &s(&[r"^(git|ls|cat)\b"]))?;
    assert!(!policy.is_empty());
    assert_eq!(policy.check(&s(&["ls", "-la"])), Ok(()));
    assert_eq!(
        policy.check(&s(&["rm", "-rf", "/"])),
        Err(CommandVetoError::Denied(r"^rm\s".to_string()))
    );
    assert_eq!(policy.check(&s(&["python", "x.py"])), Err(CommandVetoError::NotAllowed));
    assert_eq!(policy.check(&s(&["git", "log"])), Ok(()));
    Ok(())
}

#[test]
fn shell_wrapped_command_is_matched_on_the_script() -> TestResult {
    let policy = CommandPolicy::new(&s(&[r"rm\s+-rf"]), &[])?;
    assert!(policy.check(&s(&["sh", "-c", "echo hi && rm -rf /"])).is_err());
    Ok(())
}

#[test]
fn invalid_regex_is_an_error() {
    assert!(CommandPolicy::new(&s(&["("]), &[]).is_err());
    assert!(CommandPolicy::new(&[], &s(&["("])).is_err());
}

#[test]
fn empty_policy_allows() {
    let policy = CommandPolicy::default();
    assert!(policy.is_empty());
    assert_eq!(policy.check(&s(&["anything"])), Ok(()));
}
