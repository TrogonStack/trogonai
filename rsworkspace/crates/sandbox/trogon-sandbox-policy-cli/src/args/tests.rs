use super::*;
use crate::invocation::Invocation;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn parse(args: &[&str]) -> Result<Args, Box<dyn std::error::Error>> {
    let argv: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    match Invocation::parse(&argv, "0.0.0") {
        Invocation::Run(args) => Ok(args),
        other => Err(format!("not a run: {other:?}").into()),
    }
}

fn line(args: &[&str]) -> Result<CommandLine, Box<dyn std::error::Error>> {
    CommandLine::from_args(&parse(args)?).ok_or_else(|| "no command".into())
}

#[test]
fn control_fd_accepts_descriptors_from_three_up() {
    assert_eq!(ControlFd::parse("3").map(ControlFd::raw), Ok(3));
    assert_eq!(ControlFd::parse("200").map(ControlFd::raw), Ok(200));
}

#[test]
fn control_fd_refuses_everything_else() {
    for bad in ["", "abc", "2", "0", "-1", "3.5", "+4", "99999999999999999999"] {
        assert_eq!(ControlFd::parse(bad), Err(ControlFdError(bad.to_string())), "{bad:?}");
    }
}

#[test]
fn the_dash_dash_separator_ends_the_options() -> TestResult {
    let args = parse(&["--control-fd", "3", "--", "/tmp/x.sh"])?;
    assert_eq!(args.control_fd.map(ControlFd::raw), Some(3));
    assert_eq!(args.command, ["/tmp/x.sh"]);
    Ok(())
}

#[test]
fn known_flags_after_the_program_are_still_taken() -> TestResult {
    let args = parse(&["echo", "-d", "hi"])?;
    assert_eq!(args.command, ["echo", "hi"]);
    assert!(args.debug);
    Ok(())
}

#[test]
fn an_unknown_flag_stays_with_the_program_and_known_flags_are_still_taken() -> TestResult {
    let args = parse(&["ls", "-la", "-d"])?;
    assert_eq!(args.command, ["ls", "-la"]);
    assert!(args.debug);
    Ok(())
}

#[test]
fn the_command_string_wins_and_is_not_quoted() -> TestResult {
    let line = line(&["-c", "echo \"a b\" | wc -w"])?;
    assert_eq!(line.shell_text(), "echo \"a b\" | wc -w");
    assert_eq!(line.debug_line(), "Command string mode: echo \"a b\" | wc -w");
    Ok(())
}

#[test]
fn positional_words_are_quoted_for_the_shell() -> TestResult {
    let line = line(&["printf", "%s\n", "hello world"])?;
    assert_eq!(line.shell_text(), "printf '%s\n' 'hello world'");
    assert!(line.debug_line().starts_with("Original command: "));
    Ok(())
}

#[test]
fn an_empty_command_string_falls_through_to_the_words() -> TestResult {
    assert_eq!(line(&["-c", "", "echo", "hi"])?.shell_text(), "echo hi");
    assert_eq!(CommandLine::from_args(&parse(&["-c", ""])?), None);
    Ok(())
}

#[test]
fn no_command_is_none() -> TestResult {
    assert_eq!(CommandLine::from_args(&parse(&["-d"])?), None);
    Ok(())
}
