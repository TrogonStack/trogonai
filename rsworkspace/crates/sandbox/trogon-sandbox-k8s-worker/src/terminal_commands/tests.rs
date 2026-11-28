use std::process::Command;

use super::*;
use crate::test_fixtures::TestResult;

fn terminal_for(path: &std::path::Path) -> Result<TerminalId, Box<dyn std::error::Error>> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".sbx-term-"))
        .ok_or("the fixture name carries the pid file prefix")?;
    Ok(TerminalId::new(name)?)
}

/// Runs the wrapper for `id` around a command that writes `marker` and prints a line.
fn run(id: &TerminalId, marker: &std::path::Path) -> std::io::Result<std::process::Output> {
    let argv = wrapped_command(
        id,
        &[
            "sh".to_owned(),
            "-c".to_owned(),
            "printf started > \"$1\"; printf user-output".to_owned(),
            "sh".to_owned(),
            marker.display().to_string(),
        ],
        None,
        &BTreeMap::new(),
    );
    Command::new(&argv[0]).args(&argv[1..]).output()
}

#[test]
fn the_wrapper_records_the_pid_then_execs_the_command() -> TestResult {
    let id = TerminalId::new("abc-1")?;
    let argv = wrapped_command(
        &id,
        &["vim".to_owned(), "x".to_owned()],
        Some("/work"),
        &BTreeMap::new(),
    );
    assert_eq!(argv[..2], ["sh", "-c"]);
    assert!(argv[2].starts_with("cd '/work' || exit 1; export TERM='xterm-256color'; "));
    assert!(argv[2].contains("echo $$ > /tmp/.sbx-term-abc-1; exec \"$@\""));
    assert!(argv[2].ends_with("'vim' 'x'"));
    Ok(())
}

#[test]
fn a_writable_pid_file_holds_the_pid_and_the_command_runs() -> TestResult {
    let root = tempfile::tempdir()?;
    let pid_file = tempfile::Builder::new().prefix(".sbx-term-").tempfile_in("/tmp")?;
    let marker = root.path().join("started");
    let output = run(&terminal_for(pid_file.path())?, &marker)?;
    assert!(output.status.success());
    assert_eq!(output.stdout, b"user-output");
    assert_eq!(std::fs::read(&marker)?, b"started");
    assert!(std::fs::read_to_string(pid_file.path())?.trim().parse::<u32>()? > 1);
    Ok(())
}

#[test]
fn the_command_never_starts_when_the_pid_cannot_be_recorded() -> TestResult {
    let root = tempfile::tempdir()?;
    // A directory where the pid file belongs makes the redirect fail.
    let blocked = tempfile::Builder::new().prefix(".sbx-term-").tempdir_in("/tmp")?;
    let marker = root.path().join("forbidden");
    let output = run(&terminal_for(blocked.path())?, &marker)?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!marker.exists());
    Ok(())
}
