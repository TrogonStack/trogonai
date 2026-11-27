use trogon_sandbox_windows_fence::{SandboxUser, SandboxUserError, SublayerGuid, SublayerGuidError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_blank_sublayer_guid_is_rejected() {
    assert_eq!(SublayerGuid::new(" "), Err(SublayerGuidError::Empty));
    assert_eq!(SublayerGuid::new(""), Err(SublayerGuidError::Empty));
}

#[test]
fn a_blank_sandbox_user_is_rejected() {
    assert_eq!(SandboxUser::new("\t"), Err(SandboxUserError::Empty));
    assert_eq!("".parse::<SandboxUser>(), Err(SandboxUserError::Empty));
}

#[test]
fn a_name_parses_and_displays() -> TestResult {
    let user = "trogon-sandbox".parse::<SandboxUser>()?;
    assert_eq!(user.as_str(), "trogon-sandbox");
    assert_eq!(user.to_string(), "trogon-sandbox");
    let guid: SublayerGuid = "{abc}".parse()?;
    assert_eq!(guid.as_str(), "{abc}");
    assert_eq!(guid.to_string(), "{abc}");
    Ok(())
}

#[test]
fn the_errors_say_what_must_not_be_empty() {
    assert_eq!(SandboxUserError::Empty.to_string(), "sandbox user must not be empty");
    assert_eq!(SublayerGuidError::Empty.to_string(), "sublayer GUID must not be empty");
}
