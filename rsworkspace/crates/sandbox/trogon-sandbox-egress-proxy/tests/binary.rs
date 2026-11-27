use std::error::Error;
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

fn proxy() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-egress-proxyd"));
    command.env_clear();
    command
}

#[test]
fn an_unknown_flag_stops_the_process_with_a_failure() -> TestResult {
    let output = proxy().arg("--no-such-flag").output()?;

    assert_eq!(output.status.code(), Some(1));
    Ok(())
}

#[test]
fn conflicting_listen_flags_stop_the_process_with_a_failure() -> TestResult {
    let output = proxy().args(["--listen", "127.0.0.1:0", "--listen-fd", "7"]).output()?;

    assert_eq!(output.status.code(), Some(1));
    Ok(())
}

#[test]
fn a_process_without_any_configuration_stops_with_a_failure() -> TestResult {
    let output = proxy().output()?;

    assert_eq!(output.status.code(), Some(1));
    Ok(())
}
