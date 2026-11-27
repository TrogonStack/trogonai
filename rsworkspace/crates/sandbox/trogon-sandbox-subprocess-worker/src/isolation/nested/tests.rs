use super::*;

#[test]
fn the_wrapped_command_keeps_the_payload_after_the_separator() -> Result<(), Box<dyn std::error::Error>> {
    let (binary, args) = wrap_command(
        "/bin/sh",
        &["-c".to_string(), "true".to_string()],
        SeccompOptions::default(),
    );
    assert_eq!(binary, GUEST_INIT_BINARY);
    assert_eq!(args, [NESTED_FLAG, "--", "/bin/sh", "-c", "true"]);
    Ok(())
}

#[test]
fn the_seccomp_options_travel_with_the_command() -> Result<(), Box<dyn std::error::Error>> {
    let (_, args) = wrap_command(
        "/bin/true",
        &[],
        SeccompOptions {
            allow_unix_sockets: false,
            allow_nested_namespaces: true,
        },
    );
    let parsed = NestedArgs::try_parse_from(std::iter::once("worker".to_string()).chain(args))?;
    assert!(parsed.seccomp.allow_nested_namespaces);
    assert!(!parsed.seccomp.allow_unix_sockets);
    assert_eq!(parsed.command, ["/bin/true"]);
    Ok(())
}

#[test]
fn only_the_flag_in_first_position_is_an_invocation() -> Result<(), Box<dyn std::error::Error>> {
    let argv = |parts: &[&str]| -> Vec<std::ffi::OsString> { parts.iter().map(Into::into).collect() };
    assert!(is_nested_invocation(&argv(&["worker", "--nested"])));
    assert!(!is_nested_invocation(&argv(&["worker", "--relay"])));
    Ok(())
}

#[test]
fn a_signal_death_is_reported_as_the_shell_does() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(exit_code(libc::SIGKILL), 128 + libc::SIGKILL);
    assert_eq!(exit_code(3 << 8), 3);
    Ok(())
}
