use std::str::FromStr;

use serde_json::json;
use trogon_sandbox_contract::{SandboxGroupName, SandboxUserName, UnixNameError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn unix_names_accept_the_useradd_grammar() -> TestResult {
    let longest = "a".repeat(32);
    for value in ["root", "_svc", "agent-a", "a1_b-2", longest.as_str()] {
        let user = SandboxUserName::new(value)?;
        assert_eq!(user.as_str(), value);
        assert_eq!(user.to_string(), value);
        assert_eq!(SandboxUserName::from_str(value)?, user);
        assert_eq!(SandboxUserName::try_from(value.to_owned())?, user);
        assert_eq!(serde_json::to_value(&user)?, json!(value));
        assert_eq!(serde_json::from_value::<SandboxUserName>(json!(value))?, user);
        assert_eq!(String::from(user), value);

        let group = SandboxGroupName::new(value)?;
        assert_eq!(group.as_str(), value);
        assert_eq!(group.to_string(), value);
        assert_eq!(SandboxGroupName::from_str(value)?, group);
        assert_eq!(SandboxGroupName::try_from(value.to_owned())?, group);
        assert_eq!(serde_json::to_value(&group)?, json!(value));
        assert_eq!(serde_json::from_value::<SandboxGroupName>(json!(value))?, group);
        assert_eq!(String::from(group), value);
    }
    Ok(())
}

#[test]
fn unix_names_reject_everything_else() {
    let too_long = "a".repeat(33);
    for (value, expected) in [
        ("", UnixNameError::InvalidLength),
        (too_long.as_str(), UnixNameError::InvalidLength),
        ("Root", UnixNameError::InvalidCharacters),
        ("1abc", UnixNameError::InvalidCharacters),
        ("-abc", UnixNameError::InvalidCharacters),
        ("agent.a", UnixNameError::InvalidCharacters),
        ("agent a", UnixNameError::InvalidCharacters),
        ("a;rm", UnixNameError::InvalidCharacters),
        ("agentA", UnixNameError::InvalidCharacters),
        ("\u{e9}t\u{e9}", UnixNameError::InvalidCharacters),
        ("a\u{e9}", UnixNameError::InvalidCharacters),
    ] {
        assert_eq!(SandboxUserName::new(value).err(), Some(expected), "{value:?}");
        assert_eq!(SandboxGroupName::new(value).err(), Some(expected), "{value:?}");

        let user_error = serde_json::from_value::<SandboxUserName>(json!(value)).err();
        assert_eq!(user_error.map(|error| error.to_string()), Some(expected.to_string()));
        let group_error = serde_json::from_value::<SandboxGroupName>(json!(value)).err();
        assert_eq!(group_error.map(|error| error.to_string()), Some(expected.to_string()));
    }
    assert_eq!(UnixNameError::InvalidLength.to_string(), "name must be 1-32 characters");
    assert_eq!(
        UnixNameError::InvalidCharacters.to_string(),
        "name must start with [a-z_] and contain only [a-z0-9_-]"
    );
}
