use super::*;

fn failure(command: &mut Command, kind: ErrorKind) -> CommandOutput {
    spawn_failure(command, &Error::from(kind))
}

#[test]
fn a_missing_directory_reads_like_a_failed_cd() {
    let mut command = Command::new("true");
    command.current_dir("/definitely/not/a/directory");
    let output = failure(&mut command, ErrorKind::NotFound);
    assert_eq!(output.exit_code, 1);
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "cd: /definitely/not/a/directory: No such file or directory"
    );
}

#[test]
fn a_missing_program_is_127() {
    let output = failure(&mut Command::new("nope"), ErrorKind::NotFound);
    assert_eq!(output.exit_code, 127);
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("failed to start command"));
}

#[test]
fn any_other_failure_is_126() {
    let mut command = Command::new("nope");
    command.current_dir("/");
    assert_eq!(failure(&mut command, ErrorKind::PermissionDenied).exit_code, 126);
}
