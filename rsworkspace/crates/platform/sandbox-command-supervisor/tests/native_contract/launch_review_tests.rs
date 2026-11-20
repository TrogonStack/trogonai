use super::*;

fn path_case(
    executable: &str,
    search: impl FnOnce(&Path) -> String,
    expected_success: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let marker = directory.path().join("executed");
    let tools = directory.path().join("tools");
    std::fs::create_dir(&tools)?;
    for worker in [
        directory.path().join("launch-review-worker"),
        tools.join("launch-review-worker"),
    ] {
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_supervisor-test-fixture"), worker)?;
    }
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
    path_case(
        "launch-review-worker",
        |cwd| cwd.join("unselected").to_string_lossy().into_owned(),
        false,
    )
}

#[test]
fn explicit_and_path_selected_working_directory_executables_remain_held_until_release()
-> Result<(), Box<dyn std::error::Error>> {
    path_case("./launch-review-worker", |_| "/usr/bin:/bin".into(), true)?;
    path_case("launch-review-worker", |_| String::new(), true)?;
    path_case("launch-review-worker", |_| ":/usr/bin".into(), true)?;
    path_case("launch-review-worker", |_| "tools".into(), true)?;
    path_case(
        "launch-review-worker",
        |cwd| cwd.join("tools").to_string_lossy().into_owned(),
        true,
    )
}
