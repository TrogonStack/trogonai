use super::*;
use std::os::unix::ffi::OsStringExt;

fn path_case(
    executable: &str,
    search: impl FnOnce(&Path) -> String,
    expected_success: bool,
    in_tools: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let marker = directory.path().join("executed");
    let tools = directory.path().join("tools");
    std::fs::create_dir(&tools)?;
    let fixture = std::fs::canonicalize(env!("CARGO_BIN_EXE_supervisor-test-fixture"))?;
    let worker = if in_tools { &tools } else { directory.path() };
    std::os::unix::fs::symlink(&fixture, worker.join("launch-review-worker"))?;
    let mut helper = Harness::start();
    helper.send(Command::Init {
        version: VERSION,
        execution_id: helper.execution.clone(),
        argv: vec![
            executable.into(),
            "effect".into(),
            marker.to_string_lossy().into_owned(),
        ],
        env: workload_environment(BTreeMap::from([("PATH".into(), search(directory.path()))])),
        cwd: Some(directory.path().to_string_lossy().into_owned()),
        timeout_ms: None,
        release_timeout_ms: None,
    });
    assert!(matches!(helper.next(), Event::Ready { .. }));
    assert!(!marker.exists());
    helper.release();
    let mut exec_failed = false;
    let mut root_exited = false;
    let until = Instant::now() + WAIT;
    loop {
        assert!(Instant::now() < until, "bounded launch settlement");
        let event = helper.next();
        if let Event::FamilyAbsent {
            root_exit_code,
            root_signal,
            released,
            reaped_count,
            ..
        } = event
        {
            assert_eq!(root_exit_code, Some(if expected_success { 7 } else { 127 }));
            assert_eq!(root_signal, None);
            assert_eq!(released, ReleaseState::Released);
            assert_eq!(reaped_count, 1);
            break;
        }
        match event {
            Event::Error {
                code: ErrorCode::ExecFailed,
                uncertain: Certainty::Certain,
                ..
            } => {
                assert!(!expected_success && !exec_failed);
                exec_failed = true;
            }
            Event::RootExited { exit_code, signal, .. } => {
                assert_eq!(exit_code, Some(if expected_success { 7 } else { 127 }));
                assert_eq!(signal, None);
                root_exited = true;
            }
            _ => return Err(std::io::Error::other("unexpected native launch event").into()),
        }
    }
    assert!(root_exited);
    assert_eq!(exec_failed, !expected_success);
    assert_eq!(marker.exists(), expected_success);
    helper.acknowledge();
    Ok(())
}

#[test]
fn a_path_miss_never_executes_an_unselected_working_directory_binary() -> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    path_case(
        "launch-review-worker",
        |cwd| cwd.join("unselected").to_string_lossy().into_owned(),
        false,
        false,
    )
}

#[test]
fn explicit_and_path_selected_working_directory_executables_remain_held_until_release()
-> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    path_case("./launch-review-worker", |_| "/usr/bin:/bin".into(), true, false)?;
    path_case("launch-review-worker", |_| String::new(), true, false)?;
    path_case("launch-review-worker", |_| ":/usr/bin".into(), true, false)?;
    path_case("launch-review-worker", |_| "tools".into(), true, true)?;
    path_case(
        "launch-review-worker",
        |cwd| cwd.join("tools").to_string_lossy().into_owned(),
        true,
        true,
    )
}

#[test]
fn inherited_non_unicode_working_directory_preserves_the_selected_executable_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    let _custody = TestCustody::enter();
    let directory = tempfile::tempdir()?;
    let cwd = directory
        .path()
        .join(std::ffi::OsString::from_vec(b"work-\xff".to_vec()));
    let decoy = directory.path().join("work-\u{fffd}");
    std::fs::create_dir(&cwd)?;
    std::fs::create_dir(&decoy)?;
    let fixture = std::fs::canonicalize(env!("CARGO_BIN_EXE_supervisor-test-fixture"))?;
    std::os::unix::fs::symlink(fixture, cwd.join("worker"))?;
    std::os::unix::fs::symlink("/bin/true", decoy.join("worker"))?;
    let marker = directory.path().join("executed");
    let binary = std::fs::canonicalize(env!("CARGO_BIN_EXE_sandbox-command-supervisor"))?;
    let mut command = Process::new(binary);
    command.current_dir(&cwd);
    let mut helper = Harness::start_command(command);
    helper.send(Command::Init {
        version: VERSION,
        execution_id: helper.execution.clone(),
        argv: vec![
            "worker".into(),
            "effect".into(),
            marker.to_str().ok_or("marker path is not Unicode")?.into(),
        ],
        env: workload_environment(BTreeMap::from([("PATH".into(), ".".into())])),
        cwd: None,
        timeout_ms: None,
        release_timeout_ms: None,
    });
    assert!(matches!(helper.next(), Event::Ready { .. }));
    assert!(!marker.exists());
    helper.release();
    assert!(matches!(
        helper.next(),
        Event::RootExited {
            exit_code: Some(7),
            signal: None,
            ..
        }
    ));
    assert!(matches!(
        helper.next(),
        Event::FamilyAbsent {
            root_exit_code: Some(7),
            root_signal: None,
            released: ReleaseState::Released,
            reaped_count: 1,
            ..
        }
    ));
    assert!(marker.exists());
    helper.acknowledge();
    Ok(())
}
