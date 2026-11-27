use super::*;

#[test]
fn the_wrapped_command_keeps_the_payload_after_the_separator() -> Result<(), Box<dyn std::error::Error>> {
    let (binary, args) = InitWiring::wrap_command("/bin/sh", &["-c".to_string(), "true".to_string()]);
    assert_eq!(binary, GUEST_INIT_BINARY);
    assert_eq!(args, [INIT_FLAG, "--", "/bin/sh", "-c", "true"]);
    Ok(())
}

#[test]
fn only_the_flag_in_first_position_is_an_invocation() -> Result<(), Box<dyn std::error::Error>> {
    let argv = |parts: &[&str]| -> Vec<std::ffi::OsString> { parts.iter().map(Into::into).collect() };
    assert!(is_init_invocation(&argv(&["worker", "--guest-init"])));
    assert!(!is_init_invocation(&argv(&["worker", "--relay"])));
    assert!(!is_init_invocation(&argv(&["worker"])));
    Ok(())
}

#[test]
fn a_signal_death_is_reported_as_the_shell_does() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(exit_code(libc::SIGTERM), 128 + libc::SIGTERM);
    assert_eq!(exit_code(5 << 8), 5);
    Ok(())
}
