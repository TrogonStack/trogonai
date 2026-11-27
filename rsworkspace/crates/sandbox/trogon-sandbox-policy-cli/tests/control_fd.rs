#![cfg(unix)]
mod support;

use support::{Pipe, TestResult, Watched, finish, hand_over, policy_cli, run};

const UPDATE: &str = r#"{"network": {"allowedDomains": ["updated-domain.com"], "deniedDomains": []}, "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []}}"#;
const REJECTED: &str =
    r#"{"network": {"allowedDomains": ["secret-looking-value.example"], "deniedDomains": [], "httpProxyPort": 1}}"#;

fn debug_command(fd: i32, tail: &[&str]) -> std::process::Command {
    let mut command = policy_cli();
    command.args(["-d", "--control-fd", &fd.to_string()]).args(tail);
    command
}

#[test]
fn validation_rejects_values_that_are_not_descriptors_at_least_three() -> TestResult {
    for value in ["abc", "2", "-1", "", "3x"] {
        let result = run(&["--control-fd", value, "echo", "should-not-run"])?;
        assert_eq!(result.code(), Some(1), "{value}");
        assert!(result.stderr.contains("--control-fd"), "{value}: {}", result.stderr);
        assert!(!result.stdout.contains("should-not-run"));
    }
    Ok(())
}

#[test]
fn validation_rejects_a_descriptor_that_is_not_open() -> TestResult {
    let result = run(&["--control-fd", "200", "echo", "should-not-run"])?;
    assert_eq!(result.code(), Some(1));
    assert!(
        result.stderr.contains("--control-fd 200 is not usable"),
        "{}",
        result.stderr
    );
    assert!(!result.stdout.contains("should-not-run"));
    Ok(())
}

#[test]
fn validation_rejects_a_write_only_descriptor() -> TestResult {
    let pipe = Pipe::open()?;
    let mut command = debug_command(3, &["echo", "should-not-run"]);
    hand_over(&mut command, pipe.write, 3);
    let result = finish(command)?;
    assert_eq!(result.code(), Some(1));
    assert!(
        result.stderr.contains("--control-fd 3 is not usable"),
        "{}",
        result.stderr
    );
    assert!(!result.stdout.contains("should-not-run"));
    Ok(())
}

#[test]
fn updates_applies_a_line_sent_before_the_command_finishes() -> TestResult {
    let pipe = Pipe::open()?;
    pipe.send(&format!("{UPDATE}\n"))?;
    let mut command = debug_command(3, &["-c", "sleep 1; echo done"]);
    hand_over(&mut command, pipe.read, 3);
    let result = finish(command)?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(result.stdout.contains("done"));
    assert!(
        result.stderr.contains("Config updated from control fd"),
        "{}",
        result.stderr
    );
    assert!(result.stderr.contains("updated-domain.com"));
    Ok(())
}

#[test]
fn updates_applies_a_live_update() -> TestResult {
    let pipe = Pipe::open()?;
    let mut command = debug_command(3, &["-c", "sleep 4; echo done"]);
    hand_over(&mut command, pipe.read, 3);
    let mut watched = Watched::spawn(command)?;
    watched.wait_for_stderr("Listening for config updates on fd 3")?;
    pipe.send(&format!("{UPDATE}\n"))?;
    watched.wait_for_stderr("Config updated from control fd")?;
    let result = watched.finish()?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(result.stdout.contains("done"));
    Ok(())
}

#[test]
fn updates_ignores_invalid_json_and_keeps_going() -> TestResult {
    let pipe = Pipe::open()?;
    pipe.send(&format!("this is not json\n{UPDATE}\n"))?;
    let mut command = debug_command(3, &["-c", "sleep 1; echo done"]);
    hand_over(&mut command, pipe.read, 3);
    let result = finish(command)?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(
        result
            .stderr
            .contains("Invalid config on control fd 3: ignored, previous config still in force")
    );
    assert!(!result.stderr.contains("this is not json"));
    assert!(result.stderr.contains("Config updated from control fd"));
    Ok(())
}

#[test]
fn updates_a_rejected_update_does_not_echo_its_content() -> TestResult {
    let pipe = Pipe::open()?;
    pipe.send(&format!("{REJECTED}\n"))?;
    let mut command = debug_command(3, &["-c", "sleep 1; echo done"]);
    hand_over(&mut command, pipe.read, 3);
    let result = finish(command)?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(result.stderr.contains("Invalid config on control fd 3"));
    assert!(!result.stderr.contains("secret-looking-value"));
    Ok(())
}

#[test]
fn updates_a_rejected_update_is_reported_without_debug() -> TestResult {
    let pipe = Pipe::open()?;
    pipe.send("{ \"network\": \"not-an-object\" }\n")?;
    let mut command = policy_cli();
    command.args(["--control-fd", "3", "-c", "sleep 1; echo done"]);
    hand_over(&mut command, pipe.read, 3);
    let result = finish(command)?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(result.stdout.contains("done"));
    assert!(result.stderr.contains("Invalid config on control fd 3"));
    assert!(!result.stderr.contains("not-an-object"));
    Ok(())
}

#[test]
fn updates_blank_lines_are_ignored() -> TestResult {
    let pipe = Pipe::open()?;
    pipe.send(&format!("\n   \n{UPDATE}\n\n"))?;
    let mut command = debug_command(3, &["-c", "sleep 1; echo done"]);
    hand_over(&mut command, pipe.read, 3);
    let result = finish(command)?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(!result.stderr.contains("Invalid config"));
    assert!(result.stderr.contains("Config updated from control fd"));
    Ok(())
}

#[test]
fn updates_a_closed_channel_leaves_the_command_running() -> TestResult {
    let mut pipe = Pipe::open()?;
    pipe.send(&format!("{UPDATE}\n"))?;
    let mut command = debug_command(3, &["-c", "sleep 1; echo survived"]);
    hand_over(&mut command, pipe.read, 3);
    let mut watched = Watched::spawn(command)?;
    watched.wait_for_stderr("Config updated from control fd")?;
    pipe.close_read();
    drop(pipe);
    let result = watched.finish()?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(result.stdout.contains("survived"));
    Ok(())
}

#[test]
fn updates_works_with_a_regular_file() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("updates.jsonl");
    std::fs::write(&path, format!("{UPDATE}\n"))?;
    let file = std::fs::File::open(&path)?;
    let mut command = debug_command(3, &["-c", "sleep 1; echo done"]);
    let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
    hand_over(&mut command, fd, 3);
    let result = finish(command)?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(result.stderr.contains("Config updated from control fd"));
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn the_child_never_sees_the_control_descriptor() -> TestResult {
    let pipe = Pipe::open()?;
    let mut command = debug_command(20, &["-c", "cat /proc/self/fd/20 2>&1 | wc -c; true"]);
    hand_over(&mut command, pipe.read, 20);
    let result = finish(command)?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert_eq!(result.stdout.trim(), "0");
    Ok(())
}

#[test]
fn stdin_still_reaches_the_command_with_a_control_descriptor() -> TestResult {
    let pipe = Pipe::open()?;
    let mut command = debug_command(3, &["-c", "cat"]);
    hand_over(&mut command, pipe.read, 3);
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("the command has no stdin pipe")?;
    std::io::Write::write_all(&mut stdin, b"from-stdin\n")?;
    drop(stdin);
    let result = support::collect(child)?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(result.stdout.contains("from-stdin"), "{}", result.stdout);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn the_control_descriptor_is_out_of_reach_of_a_command_that_could_write_a_config() -> TestResult {
    let dir = tempfile::tempdir()?;
    let fifo = dir.path().join("control.fifo");
    let made = std::process::Command::new("mkfifo").arg(&fifo).status()?;
    assert!(made.success());
    let mut caller = std::fs::OpenOptions::new().read(true).write(true).open(&fifo)?;
    std::io::Write::write_all(&mut caller, format!("{UPDATE}\n").as_bytes())?;
    let injected = UPDATE.replace("updated-domain.com", "injected-by-the-%s");
    let script = format!(
        "ls -l /proc/self/fd; printf '{injected}\\n' sandbox.com >&20 || echo WRITE_REFUSED; sleep 1; echo SANDBOX_DONE"
    );
    let mut command = std::process::Command::new("bash");
    command
        .arg("-c")
        .arg(format!(
            "exec 20<>{}; exec {} -d --control-fd 20 -c \"$SCRIPT\"",
            fifo.display(),
            env!("CARGO_BIN_EXE_trogon-sandbox-policy"),
        ))
        .env("SCRIPT", script)
        .env("HOME", support::NO_HOME)
        .env_remove("TROGON_SANDBOX_POLICY_DEBUG");
    let result = finish(command)?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(result.stdout.contains("SANDBOX_DONE"), "{}", result.stdout);
    assert!(result.stdout.contains("WRITE_REFUSED"), "{}", result.stdout);
    assert!(result.stdout.contains(" 20 -> /dev/null"), "{}", result.stdout);
    assert!(
        !result.stderr.contains("injected-by-the-sandbox.com"),
        "{}",
        result.stderr
    );
    assert!(result.stderr.contains("updated-domain.com"), "{}", result.stderr);
    Ok(())
}
