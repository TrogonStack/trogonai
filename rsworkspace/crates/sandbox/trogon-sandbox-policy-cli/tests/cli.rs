#![cfg(unix)]
mod support;

use support::{NO_HOME, TestResult, Watched, finish, policy_cli, run, script, utf8};

fn with_home(home: &std::path::Path, args: &[&str]) -> Result<support::Finished, Box<dyn std::error::Error>> {
    let mut command = policy_cli();
    command.env("HOME", home).args(args);
    finish(command)
}

#[test]
fn version_is_the_package_version() -> TestResult {
    let result = run(&["--version"])?;
    assert_eq!(result.stdout.trim(), env!("CARGO_PKG_VERSION"));
    assert_eq!(result.code(), Some(0));
    Ok(())
}

#[test]
fn command_string_runs_a_simple_command() -> TestResult {
    let result = run(&["-c", "echo hello"])?;
    assert_eq!(result.stdout.trim(), "hello");
    assert_eq!(result.code(), Some(0));
    Ok(())
}

#[test]
fn command_string_passes_the_string_without_escaping() -> TestResult {
    let result = run(&["-c", "echo \"hello world\""])?;
    assert_eq!(result.stdout.trim(), "hello world");
    Ok(())
}

#[test]
fn command_string_handles_json_arguments() -> TestResult {
    let result = run(&["-c", "echo '{\"key\": \"value\"}'"])?;
    assert_eq!(result.stdout.trim(), "{\"key\": \"value\"}");
    let json = "{\"servers\":{\"name\":\"test\",\"type\":\"sdk\"}}";
    let result = run(&["-c", &format!("echo '{json}'")])?;
    assert_eq!(result.stdout.trim(), json);
    Ok(())
}

#[test]
fn command_string_expands_variables_in_the_shell() -> TestResult {
    let result = run(&["-c", "echo $HOME"])?;
    assert_ne!(result.stdout.trim(), "$HOME");
    assert_eq!(result.code(), Some(0));
    Ok(())
}

#[test]
fn command_string_runs_the_string_under_bash() -> TestResult {
    let result = run(&["-c", "echo ${BASH_VERSION:+bash}"])?;
    assert_eq!(result.stdout.trim(), "bash");
    Ok(())
}

#[test]
fn command_string_without_bash_on_the_path_the_spawn_fails() -> TestResult {
    let empty = tempfile::tempdir()?;
    #[cfg(target_os = "linux")]
    if let Some(bwrap) = std::env::split_paths(
        &trogon_std::env::ReadEnv::var_os(&trogon_std::env::SystemEnv, "PATH").unwrap_or_default(),
    )
    .map(|dir| dir.join("bwrap"))
    .find(|candidate| candidate.is_file())
    {
        std::os::unix::fs::symlink(bwrap, empty.path().join("bwrap"))?;
    }
    let mut command = policy_cli();
    command.env("PATH", empty.path()).args(["-c", "echo hi"]);
    let result = finish(command)?;
    assert!(result.stderr.contains("bash"), "{}", result.stderr);
    assert_ne!(result.code(), Some(0));
    Ok(())
}

#[test]
fn command_string_supports_pipes_and_command_substitution() -> TestResult {
    assert_eq!(run(&["-c", "echo \"hello world\" | wc -w"])?.stdout.trim(), "2");
    let result = run(&["-c", "echo \"count: $(echo 1 2 3 | wc -w)\""])?;
    assert!(result.stdout.contains('3'));
    Ok(())
}

#[test]
fn positional_runs_a_program_with_arguments() -> TestResult {
    assert_eq!(run(&["echo", "hello"])?.stdout.trim(), "hello");
    assert_eq!(run(&["echo", "hello", "world"])?.stdout.trim(), "hello world");
    Ok(())
}

#[test]
fn positional_leaves_flags_after_the_program_to_the_program() -> TestResult {
    let result = run(&["ls", "-1", "/dev/null"])?;
    assert_eq!(result.stdout.trim(), "/dev/null");
    assert_eq!(result.code(), Some(0));
    Ok(())
}

#[test]
fn positional_keeps_argument_boundaries() -> TestResult {
    let result = run(&["printf", "%s\n", "hello world", "a b"])?;
    assert_eq!(result.stdout, "hello world\na b\n");
    Ok(())
}

#[test]
fn positional_keeps_shell_metacharacters_as_data() -> TestResult {
    assert_eq!(run(&["printf", "%s", "$HOME;|&"])?.stdout, "$HOME;|&");
    Ok(())
}

#[test]
fn passes_the_exit_code_through() -> TestResult {
    assert_eq!(run(&["-c", "exit 7"])?.code(), Some(7));
    assert_eq!(run(&["-c", "true"])?.code(), Some(0));
    Ok(())
}

#[test]
fn a_missing_program_exits_127() -> TestResult {
    let result = run(&["definitely-not-a-program-trogon-sandbox-policy"])?;
    assert_eq!(result.code(), Some(127));
    Ok(())
}

#[test]
fn errors_no_command_is_refused() -> TestResult {
    let result = run(&[])?;
    assert!(result.stderr.contains("No command specified"));
    assert_eq!(result.code(), Some(1));
    Ok(())
}

#[test]
fn errors_an_empty_command_string_is_no_command() -> TestResult {
    let result = run(&["-c", ""])?;
    assert!(result.stderr.contains("No command specified"));
    assert_eq!(result.code(), Some(1));
    Ok(())
}

#[test]
fn errors_an_empty_command_string_falls_through_to_the_words() -> TestResult {
    let result = run(&["-c", "", "echo", "words"])?;
    assert_eq!(result.stdout.trim(), "words");
    assert_eq!(result.code(), Some(0));
    Ok(())
}

#[test]
fn errors_settings_are_checked_before_the_missing_command() -> TestResult {
    let result = run(&["--settings", "/nonexistent/sandbox-settings.json"])?;
    assert!(result.stderr.contains("does not exist"));
    assert!(!result.stderr.contains("No command specified"));
    assert_eq!(result.code(), Some(1));
    Ok(())
}

#[test]
fn errors_only_options_is_refused() -> TestResult {
    let result = run(&["-d"])?;
    assert!(result.stderr.contains("No command specified"));
    assert_eq!(result.code(), Some(1));
    Ok(())
}

#[test]
fn settings_flag_a_missing_explicit_file_is_refused() -> TestResult {
    let result = run(&[
        "--settings",
        "/nonexistent/sandbox-settings.json",
        "echo",
        "should-not-run",
    ])?;
    assert_eq!(result.code(), Some(1));
    assert!(result.stderr.contains("does not exist"));
    assert!(result.stderr.contains("--settings asked for"));
    assert!(!result.stdout.contains("should-not-run"));
    Ok(())
}

#[test]
fn settings_flag_an_invalid_explicit_file_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("invalid.json");
    std::fs::write(&path, r#"{"network": {}}"#)?;
    let result = run(&["--settings", utf8(&path)?, "echo", "should-not-run"])?;
    assert_eq!(result.code(), Some(1));
    assert!(result.stderr.contains("does not hold a valid config"));
    assert!(!result.stdout.contains("should-not-run"));
    Ok(())
}

#[test]
fn settings_flag_an_option_the_runtime_cannot_enforce_is_refused_by_name() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("weaker.json");
    std::fs::write(
        &path,
        r#"{"network": {"allowedDomains": [], "deniedDomains": []}, "socatPath": "socat",
            "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []}}"#,
    )?;
    let result = run(&["--settings", utf8(&path)?, "echo", "should-not-run"])?;
    assert_eq!(result.code(), Some(1));
    assert!(result.stderr.contains("socatPath"), "{}", result.stderr);
    assert!(!result.stdout.contains("should-not-run"));
    Ok(())
}

#[test]
fn settings_flag_a_valid_explicit_file_is_used() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("ok.json");
    std::fs::write(
        &path,
        r#"{"network": {"allowedDomains": ["example.com"], "deniedDomains": []},
            "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []}}"#,
    )?;
    let result = run(&["-s", utf8(&path)?, "echo", "configured"])?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(result.stdout.contains("configured"));
    Ok(())
}

fn home_with(text: &str) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let home = tempfile::tempdir()?;
    std::fs::write(home.path().join(".sandbox-settings.json"), text)?;
    Ok(home)
}

#[test]
fn default_settings_file_an_invalid_file_is_refused_and_names_the_key() -> TestResult {
    let home = home_with(r#"{"network": {}}"#)?;
    let result = with_home(home.path(), &["echo", "should-not-run"])?;
    assert_eq!(result.code(), Some(1));
    assert!(result.stderr.contains("does not hold a valid config"));
    assert!(result.stderr.contains("network.allowedDomains"));
    assert!(!result.stdout.contains("should-not-run"));
    Ok(())
}

#[test]
fn default_settings_file_an_empty_or_blank_file_is_refused() -> TestResult {
    for text in ["", "   \n\t  "] {
        let home = home_with(text)?;
        let result = with_home(home.path(), &["echo", "should-not-run"])?;
        assert_eq!(result.code(), Some(1));
        assert!(result.stderr.contains("is empty"));
        assert!(!result.stdout.contains("should-not-run"));
    }
    Ok(())
}

#[test]
fn default_settings_file_a_path_that_cannot_be_read_is_refused() -> TestResult {
    let home = tempfile::tempdir()?;
    std::fs::create_dir(home.path().join(".sandbox-settings.json"))?;
    let result = with_home(home.path(), &["echo", "should-not-run"])?;
    assert_eq!(result.code(), Some(1));
    assert!(result.stderr.contains("could not be read"));
    assert!(result.stderr.contains("EISDIR"));
    assert!(!result.stdout.contains("should-not-run"));
    Ok(())
}

#[test]
fn default_settings_file_no_file_means_the_built_in_settings() -> TestResult {
    let home = tempfile::tempdir()?;
    let result = with_home(home.path(), &["echo", "no-settings-file"])?;
    assert_eq!(result.code(), Some(0));
    assert!(result.stdout.contains("no-settings-file"));
    let result = with_home(std::path::Path::new(NO_HOME), &["echo", "no-home"])?;
    assert_eq!(result.code(), Some(0));
    Ok(())
}

#[test]
fn debug_output_the_environment_variable_enables_it() -> TestResult {
    let mut command = policy_cli();
    command
        .env("TROGON_SANDBOX_POLICY_DEBUG", "true")
        .args(["echo", "test"]);
    let result = finish(command)?;
    assert!(result.stderr.contains("[SandboxDebug]"));
    assert!(result.stderr.contains("Original command"));
    assert_eq!(result.code(), Some(0));

    let mut command = policy_cli();
    command
        .env("TROGON_SANDBOX_POLICY_DEBUG", "true")
        .args(["-c", "echo test"]);
    let result = finish(command)?;
    assert!(result.stderr.contains("[SandboxDebug]"));
    assert!(result.stderr.contains("Command string mode"));
    Ok(())
}

#[test]
fn debug_output_any_non_empty_value_enables_it() -> TestResult {
    let mut command = policy_cli();
    command.env("TROGON_SANDBOX_POLICY_DEBUG", "0").args(["echo", "test"]);
    let result = finish(command)?;
    assert!(result.stderr.contains("[SandboxDebug]"));
    assert!(result.stderr.contains("Initializing sandbox..."));

    let mut command = policy_cli();
    command.env("TROGON_SANDBOX_POLICY_DEBUG", "").args(["echo", "test"]);
    assert!(!finish(command)?.stderr.contains("[SandboxDebug]"));
    Ok(())
}

#[test]
fn debug_output_nothing_is_logged_without_it() -> TestResult {
    let result = run(&["echo", "test"])?;
    assert!(!result.stderr.contains("[SandboxDebug]"));
    assert_eq!(result.code(), Some(0));
    Ok(())
}

#[test]
fn debug_output_the_flag_enables_it_without_the_variable() -> TestResult {
    let result = run(&["--debug", "echo", "test"])?;
    assert!(result.stderr.contains("[SandboxDebug]"));
    assert_eq!(result.code(), Some(0));
    Ok(())
}

fn send(pid: u32, signal: i32) -> TestResult {
    let pid = i32::try_from(pid)?;
    // SAFETY: `kill` takes plain integers and touches no memory.
    if unsafe { libc::kill(pid, signal) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

#[test]
fn signals_terminate_is_forwarded_to_the_running_command() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ready = dir.path().join("ready");
    let mut command = policy_cli();
    command.current_dir(dir.path()).args([
        "-d",
        "-s",
        &writable_settings(dir.path())?,
        "-c",
        "touch ready; exec sleep 60",
    ]);
    let mut watched = Watched::spawn(command)?;
    watched.wait_for_stderr("Command string mode")?;
    wait_for_file(&ready)?;
    send(watched.child.id(), libc::SIGTERM)?;
    let result = watched.finish()?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn signals_an_interrupt_before_the_command_starts_stops_the_run() -> TestResult {
    let dir = tempfile::tempdir()?;
    let settings = dir.path().join("settings.json");
    let made = std::process::Command::new("mkfifo").arg(&settings).status()?;
    assert!(made.success());
    let mut command = policy_cli();
    command
        .current_dir(dir.path())
        .args(["-d", "-s", &settings.display().to_string(), "-c", "echo RAN"]);
    let watched = Watched::spawn(command)?;
    wait_for_sigint_handler(watched.child.id())?;
    send(watched.child.id(), libc::SIGINT)?;
    let mut fifo = std::fs::OpenOptions::new().write(true).open(&settings)?;
    std::io::Write::write_all(&mut fifo, WRITABLE_SETTINGS.as_bytes())?;
    let result = watched.finish()?;
    assert!(result.stderr.contains("interrupted by SIGINT"), "{}", result.stderr);
    assert!(!result.stdout.contains("RAN"), "{}", result.stdout);
    assert_eq!(result.code(), Some(1), "{}", result.stderr);
    Ok(())
}

#[test]
fn signals_a_command_killed_by_another_signal_is_reported() -> TestResult {
    let result = run(&["-c", "kill -KILL $$"])?;
    if cfg!(target_os = "macos") {
        assert_eq!(result.code(), Some(1));
        assert!(result.stderr.contains("Process killed by signal: SIGKILL"));
    } else {
        assert!(matches!(result.code(), Some(1) | Some(137)), "{:?}", result.status);
    }
    Ok(())
}

fn writable_settings(dir: &std::path::Path) -> Result<String, Box<dyn std::error::Error>> {
    let path = dir.join("settings.json");
    std::fs::write(
        &path,
        r#"{"network": {"allowedDomains": [], "deniedDomains": []},
            "filesystem": {"denyRead": [], "allowWrite": ["."], "denyWrite": []}}"#,
    )?;
    Ok(path.display().to_string())
}

#[cfg(target_os = "linux")]
const WRITABLE_SETTINGS: &str = r#"{"network": {"allowedDomains": [], "deniedDomains": []},
    "filesystem": {"denyRead": [], "allowWrite": ["."], "denyWrite": []}}"#;

#[cfg(target_os = "linux")]
fn wait_for_sigint_handler(pid: u32) -> TestResult {
    const SIGINT_BIT: u64 = 1 << (libc::SIGINT - 1);
    let started = std::time::Instant::now();
    loop {
        let caught = std::fs::read_to_string(format!("/proc/{pid}/status"))
            .ok()
            .and_then(|status| {
                status
                    .lines()
                    .find_map(|line| line.strip_prefix("SigCgt:"))
                    .and_then(|mask| u64::from_str_radix(mask.trim(), 16).ok())
            })
            .is_some_and(|mask| mask & SIGINT_BIT != 0);
        if caught {
            return Ok(());
        }
        if started.elapsed() >= support::EXIT_TIMEOUT {
            return Err("the command never installed its SIGINT handler".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn wait_for_file(path: &std::path::Path) -> TestResult {
    let started = std::time::Instant::now();
    while !path.exists() {
        if started.elapsed() >= support::EXIT_TIMEOUT {
            return Err(format!("{path:?} never appeared").into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(())
}

#[test]
fn signals_script_helper_writes_an_executable() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = script(dir.path(), "echo hi")?;
    assert!(std::path::Path::new(&path).exists());
    Ok(())
}

fn settings_with(dir: &std::path::Path, allow_write: &str) -> Result<String, Box<dyn std::error::Error>> {
    let path = dir.join("settings.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"network": {{"allowedDomains": [], "deniedDomains": []}},
                "filesystem": {{"denyRead": [], "allowWrite": {allow_write}, "denyWrite": []}}}}"#
        ),
    )?;
    Ok(path.display().to_string())
}

fn in_dir(dir: &std::path::Path, args: &[&str]) -> Result<support::Finished, Box<dyn std::error::Error>> {
    let mut command = policy_cli();
    command.current_dir(dir).args(args);
    finish(command)
}

#[test]
fn host_view_runs_in_the_callers_directory_with_the_inherited_environment() -> TestResult {
    let dir = tempfile::tempdir()?;
    let cwd = dir.path().canonicalize()?;
    let result = in_dir(&cwd, &["-c", "pwd; echo $TROGON_SANDBOXED; echo $HOME"])?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    let lines: Vec<&str> = result.stdout.lines().collect();
    assert_eq!(lines[0], utf8(&cwd)?);
    assert_eq!(lines[1], "1");
    assert_eq!(lines[2], NO_HOME);
    Ok(())
}

#[test]
fn host_view_writes_nothing_without_an_allowed_path() -> TestResult {
    let dir = tempfile::tempdir()?;
    let cwd = dir.path().canonicalize()?;
    let result = in_dir(&cwd, &["-c", "echo no > file.txt"])?;
    assert_ne!(result.code(), Some(0));
    assert!(!cwd.join("file.txt").exists());
    Ok(())
}

#[test]
fn host_view_a_relative_allowed_path_is_relative_to_the_callers_directory() -> TestResult {
    let dir = tempfile::tempdir()?;
    let cwd = dir.path().canonicalize()?;
    std::fs::create_dir(cwd.join("out"))?;
    let settings = settings_with(&cwd, r#"["./out"]"#)?;
    let result = in_dir(&cwd, &["-s", &settings, "-c", "echo yes > out/a.txt; echo no > b.txt"])?;
    assert_ne!(result.code(), Some(0));
    assert_eq!(std::fs::read_to_string(cwd.join("out/a.txt"))?, "yes\n");
    assert!(!cwd.join("b.txt").exists());
    Ok(())
}

#[test]
fn host_view_the_guest_view_flag_keeps_absolute_sandbox_paths() -> TestResult {
    let dir = tempfile::tempdir()?;
    let cwd = dir.path().canonicalize()?;
    let settings = settings_with(&cwd, r#"["relative"]"#)?;
    let result = in_dir(&cwd, &["--guest-view", "-s", &settings, "echo", "x"])?;
    assert_eq!(result.code(), Some(1));
    assert!(result.stderr.contains("filesystem.allowWrite.0"), "{}", result.stderr);
    Ok(())
}

const MASKING: &str = r#"{
    "network": {"allowedDomains": ["example.com"], "deniedDomains": [], "tlsTerminate": {}},
    "filesystem": {"denyRead": [], "allowWrite": [], "denyWrite": []},
    "credentials": {"envVars": [
        {"name": "MASKED_TOKEN", "mode": "mask"},
        {"name": "WITHHELD_TOKEN", "mode": "deny"}]}
}"#;

#[test]
fn credentials_masked_and_denied_variables_never_reach_the_command_in_the_clear() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("masking.json");
    std::fs::write(&path, MASKING)?;
    let mut command = policy_cli();
    command
        .env("MASKED_TOKEN", "real-masked-value")
        .env("WITHHELD_TOKEN", "real-withheld-value")
        .args([
            "-s",
            utf8(&path)?,
            "-c",
            "echo masked=$MASKED_TOKEN withheld=$WITHHELD_TOKEN",
        ]);
    let result = finish(command)?;
    assert_eq!(result.code(), Some(0), "{}", result.stderr);
    assert!(!result.stdout.contains("real-masked-value"), "{}", result.stdout);
    assert!(!result.stdout.contains("real-withheld-value"), "{}", result.stdout);
    assert!(result.stdout.contains("masked=fake_value_"), "{}", result.stdout);
    Ok(())
}
