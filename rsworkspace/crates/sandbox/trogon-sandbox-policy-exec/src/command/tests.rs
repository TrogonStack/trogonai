use super::*;

#[test]
fn a_shell_command_is_attributed_by_its_script() {
    assert_eq!(
        CommandSpec::shell("echo hi").attribution(),
        CommandAttribution::new("echo hi")
    );
}

#[test]
fn a_command_id_wins_over_the_command_line() {
    let mut spec = CommandSpec::shell("echo hi");
    spec.command_id = Some(CommandId::new("0123456789abcdefghijkl"));
    assert_eq!(spec.attribution(), CommandAttribution::new("0123456789abcdefghijkl"));
}

#[test]
fn without_a_command_line_the_program_and_arguments_name_it() {
    assert_eq!(
        CommandSpec::new("/bin/echo").arg("hi").attribution(),
        CommandAttribution::new("/bin/echo hi")
    );
}
