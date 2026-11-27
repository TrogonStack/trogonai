use super::*;
use crate::support_tests::TestResult;

#[test]
fn wrapper_records_the_pid_then_execs_the_command() -> TestResult {
    let id = TerminalId::new("abc-1")?;
    let argv = wrapped_command(&id, &[TerminalArgument::new("vim")?, TerminalArgument::new("x")?]);
    assert_eq!(argv[0..2], ["sh", "-c"]);
    assert_eq!(argv[2], "echo $$ > /tmp/.sbx-term-abc-1; exec \"$@\"");
    assert_eq!(argv[4..], ["vim", "x"]);
    Ok(())
}

#[test]
fn an_empty_command_starts_a_shell() -> TestResult {
    let argv = wrapped_command(&TerminalId::new("t")?, &[]);
    assert_eq!(argv.last().map(String::as_str), Some("/bin/sh"));
    Ok(())
}

#[test]
fn term_defaults_but_never_overrides() {
    let mut env = std::collections::BTreeMap::new();
    assert_eq!(exec_env(&env), vec!["TERM=xterm-256color".to_string()]);
    env.insert("TERM".to_string(), "vt100".to_string());
    assert_eq!(exec_env(&env), vec!["TERM=vt100".to_string()]);
}
