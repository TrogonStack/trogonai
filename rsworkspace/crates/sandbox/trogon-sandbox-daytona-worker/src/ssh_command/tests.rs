use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_command_with_a_user_and_a_port_names_all_three() -> TestResult {
    let command: SshCommand = "ssh -p 2222 tok@gateway.test".parse()?;
    assert_eq!(command.user.as_deref(), Some("tok"));
    assert_eq!(command.host, "gateway.test");
    assert_eq!(command.port, 2222);
    Ok(())
}

#[test]
fn the_port_defaults_to_22_and_the_user_is_optional() -> TestResult {
    let command: SshCommand = "ssh tok@gateway.test".parse()?;
    assert_eq!(command.port, 22);
    let bare: SshCommand = "ssh -p2200 gateway.test".parse()?;
    assert_eq!(bare.user, None);
    assert_eq!(bare.host, "gateway.test");
    assert_eq!(bare.port, 2200);
    Ok(())
}

#[test]
fn other_options_are_skipped() -> TestResult {
    let command: SshCommand = "ssh -o StrictHostKeyChecking=no -p 2222 tok@gateway.test".parse()?;
    assert_eq!(command.host, "gateway.test");
    Ok(())
}

#[test]
fn a_command_that_is_not_ssh_is_refused() {
    assert_eq!("scp a b".parse::<SshCommand>(), Err(SshCommandError::NotSsh));
    assert_eq!("".parse::<SshCommand>(), Err(SshCommandError::NotSsh));
}

#[test]
fn a_missing_or_malformed_port_is_refused() {
    assert_eq!("ssh -p".parse::<SshCommand>(), Err(SshCommandError::MissingPort));
    assert_eq!(
        "ssh -p high tok@gateway.test".parse::<SshCommand>(),
        Err(SshCommandError::InvalidPort("high".to_owned()))
    );
    assert_eq!(
        "ssh -p 70000 tok@gateway.test".parse::<SshCommand>(),
        Err(SshCommandError::InvalidPort("70000".to_owned()))
    );
}

#[test]
fn a_missing_host_is_refused() {
    assert_eq!("ssh tok@".parse::<SshCommand>(), Err(SshCommandError::MissingHost));
    assert_eq!("ssh -p 22".parse::<SshCommand>(), Err(SshCommandError::MissingHost));
}
