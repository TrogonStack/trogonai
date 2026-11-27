#![cfg(feature = "server")]

use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use trogon_sandbox_actor_agent::{LeaseBinding, TerminalSessions};
use trogon_sandbox_contract::{LeaseId, TerminalError, TerminalOutput, TerminalRequest, TerminalResponse};
use trogon_sandbox_protocol::{MAX_TERMINALS_PER_LEASE, terminal_output_bytes, terminal_output_seq};

type TestResult = Result<(), Box<dyn Error>>;

fn binding() -> LeaseBinding {
    LeaseBinding {
        lease_id: LeaseId::new("01J0"),
        env: BTreeMap::new(),
        working_dir: None,
        exposed_port: None,
    }
}

fn request(id: &str, op: Value) -> Result<TerminalRequest, Box<dyn Error>> {
    let mut body = json!({"lease_id": "01J0", "terminal_id": id});
    if let (Some(target), Some(op)) = (body.as_object_mut(), op.as_object()) {
        target.extend(op.clone());
    }
    Ok(serde_json::from_value(body)?)
}

fn open_request(id: &str, command: &[&str]) -> Result<TerminalRequest, Box<dyn Error>> {
    request(id, json!({"op": "open", "cols": 80, "rows": 24, "cmd": command}))
}

fn input(id: &str, text: &str) -> Result<TerminalRequest, Box<dyn Error>> {
    request(id, json!({"op": "input", "data": STANDARD.encode(text)}))
}

fn shell(script: &str) -> [&str; 3] {
    ["/bin/sh", "-c", script]
}

async fn next(frames: &mut mpsc::Receiver<TerminalOutput>) -> Result<TerminalOutput, Box<dyn Error>> {
    let frame = tokio::time::timeout(Duration::from_secs(20), frames.recv()).await?;
    frame.ok_or_else(|| "the terminal stream ended without an exit frame".into())
}

/// Reads until the exit frame and returns the output and the exit code.
async fn until_exit(frames: &mut mpsc::Receiver<TerminalOutput>) -> Result<(String, Option<i32>), Box<dyn Error>> {
    let mut output = Vec::new();
    let mut expected = None;
    loop {
        let frame = next(frames).await?;
        match frame {
            TerminalOutput::Exit { seq, exit_code } => {
                if let Some(expected) = expected {
                    assert_eq!(seq, expected, "the exit frame continues the sequence");
                }
                return Ok((String::from_utf8_lossy(&output).to_string(), exit_code));
            }
            other => {
                let Some(bytes) = terminal_output_bytes(&other) else {
                    return Err(format!("unexpected frame {other:?}").into());
                };
                output.extend(bytes);
                expected = Some(terminal_output_seq(&other) + 1);
            }
        }
    }
}

async fn until_contains(frames: &mut mpsc::Receiver<TerminalOutput>, needle: &str) -> TestResult {
    let mut seen = String::new();
    while !seen.contains(needle) {
        match next(frames).await? {
            TerminalOutput::Exit { .. } => return Err(format!("exited before `{needle}`: {seen:?}").into()),
            other => seen.push_str(&String::from_utf8_lossy(
                &terminal_output_bytes(&other).unwrap_or_default(),
            )),
        }
    }
    Ok(())
}

async fn released(sessions: &TerminalSessions) -> TestResult {
    for _ in 0..1000 {
        if sessions.open_count() == 0 {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("the terminal was never released".into())
}

#[tokio::test]
async fn output_arrives_in_order_and_the_exit_status_closes_the_stream() -> TestResult {
    let sessions = TerminalSessions::new();
    let mut frames = sessions.open(&binding(), &open_request("t1", &shell("printf hello; exit 3"))?)?;
    let (output, code) = until_exit(&mut frames).await?;
    assert!(output.contains("hello"), "got {output:?}");
    assert_eq!(code, Some(3));
    released(&sessions).await
}

#[tokio::test]
async fn input_reaches_the_command_and_close_ends_it() -> TestResult {
    let sessions = TerminalSessions::new();
    let mut frames = sessions.open(&binding(), &open_request("t1", &["/bin/cat"])?)?;
    assert_eq!(sessions.apply(&input("t1", "ping\n")?).await, TerminalResponse::Done);
    until_contains(&mut frames, "ping").await?;
    assert_eq!(
        sessions.apply(&request("t1", json!({"op": "close"}))?).await,
        TerminalResponse::Done
    );
    let (_, code) = until_exit(&mut frames).await?;
    assert_eq!(code, None, "a hung-up command has no exit code");
    released(&sessions).await?;
    assert_eq!(
        sessions.apply(&request("t1", json!({"op": "close"}))?).await,
        TerminalResponse::Done
    );
    Ok(())
}

#[tokio::test]
async fn an_empty_command_runs_the_default_shell() -> TestResult {
    let sessions = TerminalSessions::new();
    let mut frames = sessions.open(
        &binding(),
        &request("t1", json!({"op": "open", "cols": 80, "rows": 24}))?,
    )?;
    sessions.apply(&input("t1", "exit 5\n")?).await;
    let (_, code) = until_exit(&mut frames).await?;
    assert_eq!(code, Some(5));
    Ok(())
}

#[tokio::test]
async fn a_resize_reaches_the_terminal() -> TestResult {
    let sessions = TerminalSessions::new();
    let mut frames = sessions.open(&binding(), &open_request("t1", &shell("read line; stty size"))?)?;
    let resize = request("t1", json!({"op": "resize", "cols": 100, "rows": 30}))?;
    assert_eq!(sessions.apply(&resize).await, TerminalResponse::Done);
    sessions.apply(&input("t1", "\n")?).await;
    let (output, code) = until_exit(&mut frames).await?;
    assert!(output.contains("30 100"), "got {output:?}");
    assert_eq!(code, Some(0));
    Ok(())
}

#[tokio::test]
async fn the_lease_environment_the_request_environment_and_the_working_directory_apply() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join("sub"))?;
    let mut bound = binding();
    bound.working_dir = Some(dir.path().to_string_lossy().to_string());
    bound.env.insert("FROM_LEASE".to_string(), "lease".to_string());
    let sessions = TerminalSessions::new();
    let script = "printf '%s|%s|%s|%s' \"$TERM\" \"$FROM_LEASE\" \"$FROM_CALL\" \"$(pwd)\"";
    let open = request(
        "t1",
        json!({"op": "open", "cols": 80, "rows": 24, "cmd": shell(script), "env": {"FROM_CALL": "call"}, "cwd": "sub"}),
    )?;
    let mut frames = sessions.open(&bound, &open)?;
    let (output, _) = until_exit(&mut frames).await?;
    assert!(output.starts_with("xterm-256color|lease|call|"), "got {output:?}");
    assert!(output.trim_end().ends_with("/sub"), "got {output:?}");
    Ok(())
}

#[tokio::test]
async fn a_cwd_without_a_bound_working_directory_applies_as_given() -> TestResult {
    let dir = tempfile::tempdir()?;
    let sessions = TerminalSessions::new();
    let cwd = dir.path().to_string_lossy().to_string();
    let open = request(
        "t1",
        json!({"op": "open", "cols": 80, "rows": 24, "cmd": shell("pwd"), "cwd": cwd}),
    )?;
    let mut frames = sessions.open(&binding(), &open)?;
    let (output, _) = until_exit(&mut frames).await?;
    let name = dir
        .path()
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    assert!(output.contains(&name), "got {output:?}");
    Ok(())
}

#[tokio::test]
async fn a_term_from_the_lease_is_kept() -> TestResult {
    let mut bound = binding();
    bound.env.insert("TERM".to_string(), "dumb".to_string());
    let sessions = TerminalSessions::new();
    let mut frames = sessions.open(&bound, &open_request("t1", &shell("printf %s \"$TERM\""))?)?;
    let (output, _) = until_exit(&mut frames).await?;
    assert!(output.contains("dumb"), "got {output:?}");
    Ok(())
}

#[tokio::test]
async fn a_missing_program_is_a_spawn_failure() -> TestResult {
    let sessions = TerminalSessions::new();
    let result = sessions.open(&binding(), &open_request("t1", &["/nonexistent/program"])?);
    assert!(
        matches!(result, Err(TerminalError::SpawnFailed { .. })),
        "got {result:?}"
    );
    assert_eq!(sessions.open_count(), 0);
    Ok(())
}

#[tokio::test]
async fn an_id_in_use_is_refused() -> TestResult {
    let sessions = TerminalSessions::new();
    let _frames = sessions.open(&binding(), &open_request("t1", &["/bin/cat"])?)?;
    let again = sessions.open(&binding(), &open_request("t1", &["/bin/cat"])?);
    assert!(matches!(again, Err(TerminalError::IdInUse)), "got {again:?}");
    sessions.close_all();
    released(&sessions).await
}

#[tokio::test]
async fn the_per_lease_cap_is_enforced() -> TestResult {
    let sessions = TerminalSessions::new();
    let mut open = Vec::new();
    for index in 0..MAX_TERMINALS_PER_LEASE {
        open.push(sessions.open(&binding(), &open_request(&format!("t{index}"), &["/bin/cat"])?)?);
    }
    let over = sessions.open(&binding(), &open_request("over", &["/bin/cat"])?);
    assert!(
        matches!(over, Err(TerminalError::TooManyTerminals { .. })),
        "got {over:?}"
    );
    sessions.close_all();
    released(&sessions).await
}

#[tokio::test]
async fn an_open_request_must_describe_an_open() -> TestResult {
    let sessions = TerminalSessions::new();
    let result = sessions.open(&binding(), &input("t1", "x")?);
    assert!(
        matches!(result, Err(TerminalError::InvalidRequest { .. })),
        "got {result:?}"
    );
    let applied = sessions.apply(&open_request("t1", &["/bin/cat"])?).await;
    assert!(
        matches!(applied, TerminalResponse::Failed(TerminalError::InvalidRequest { .. })),
        "got {applied:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_unusable_environment_or_directory_is_refused_before_spawning() -> TestResult {
    let sessions = TerminalSessions::new();
    for env in [json!({"": "x"}), json!({"A=B": "x"}), json!({"A": "x\u{0}"})] {
        let open = request("t1", json!({"op": "open", "cols": 80, "rows": 24, "env": env}))?;
        let result = sessions.open(&binding(), &open);
        assert!(
            matches!(result, Err(TerminalError::InvalidRequest { .. })),
            "got {result:?}"
        );
    }
    for cwd in ["", "a\u{0}b"] {
        let open = request("t1", json!({"op": "open", "cols": 80, "rows": 24, "cwd": cwd}))?;
        let result = sessions.open(&binding(), &open);
        assert!(
            matches!(result, Err(TerminalError::InvalidRequest { .. })),
            "got {result:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn operations_on_an_unknown_terminal_are_not_found() -> TestResult {
    let sessions = TerminalSessions::new();
    let missing = TerminalResponse::Failed(TerminalError::NotFound);
    assert_eq!(sessions.apply(&input("nope", "x")?).await, missing);
    let resize = request("nope", json!({"op": "resize", "cols": 10, "rows": 10}))?;
    assert_eq!(sessions.apply(&resize).await, missing);
    Ok(())
}

#[tokio::test]
async fn input_that_is_not_base64_is_an_invalid_request() -> TestResult {
    let sessions = TerminalSessions::new();
    let _frames = sessions.open(&binding(), &open_request("t1", &["/bin/cat"])?)?;
    let bad = request("t1", json!({"op": "input", "data": "!!!"}));
    assert!(
        bad.is_err(),
        "the contract refuses undecodable input before it reaches the agent"
    );
    sessions.close_all();
    released(&sessions).await
}

#[tokio::test]
async fn dropping_the_receiver_hangs_the_terminal_up() -> TestResult {
    let sessions = TerminalSessions::new();
    let frames = sessions.open(&binding(), &open_request("t1", &["/bin/cat"])?)?;
    drop(frames);
    released(&sessions).await
}

#[tokio::test]
async fn a_command_that_closes_its_terminal_without_exiting_is_hung_up() -> TestResult {
    let sessions = TerminalSessions::new();
    let script = "exec 0<&- 1>&- 2>&-; sleep 2";
    let mut frames = sessions.open(&binding(), &open_request("t1", &shell(script))?)?;
    until_exit(&mut frames).await?;
    released(&sessions).await
}

#[tokio::test]
async fn output_left_by_a_background_process_is_drained_after_the_exit() -> TestResult {
    let sessions = TerminalSessions::new();
    let mut frames = sessions.open(
        &binding(),
        &open_request("t1", &shell("(sleep 3 &) ; printf done; exit 0"))?,
    )?;
    let (output, code) = until_exit(&mut frames).await?;
    assert!(output.contains("done"), "got {output:?}");
    assert_eq!(code, Some(0));
    Ok(())
}

#[tokio::test]
async fn a_receiver_that_stops_reading_does_not_lose_frames_and_close_ends_it() -> TestResult {
    let sessions = TerminalSessions::new();
    let mut frames = sessions.open(&binding(), &open_request("t1", &["/usr/bin/yes"])?)?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        sessions.apply(&request("t1", json!({"op": "close"}))?).await,
        TerminalResponse::Done
    );
    released(&sessions).await?;
    let mut expected = 0;
    while let Some(frame) = frames.recv().await {
        if let TerminalOutput::Data { seq, .. } = frame {
            assert_eq!(seq, expected, "output is gapless");
            expected += 1;
        }
    }
    assert!(expected > 0);
    Ok(())
}

#[tokio::test]
async fn a_receiver_dropped_while_output_is_pending_releases_the_terminal() -> TestResult {
    let sessions = TerminalSessions::new();
    let frames = sessions.open(&binding(), &open_request("t1", &["/usr/bin/yes"])?)?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(frames);
    released(&sessions).await
}

#[tokio::test]
async fn a_command_that_ignores_the_hangup_is_killed_after_the_grace() -> TestResult {
    let sessions = TerminalSessions::new();
    let script = "trap '' HUP; printf ready; while :; do sleep 1; done";
    let mut frames = sessions.open(&binding(), &open_request("t1", &shell(script))?)?;
    until_contains(&mut frames, "ready").await?;
    sessions.close_all();
    let (_, code) = until_exit(&mut frames).await?;
    assert_eq!(code, None);
    released(&sessions).await
}

#[tokio::test]
async fn sessions_are_shared_across_handles() -> TestResult {
    let sessions = TerminalSessions::new();
    let other = Arc::clone(&sessions);
    let _frames = sessions.open(&binding(), &open_request("t1", &["/bin/cat"])?)?;
    assert_eq!(other.open_count(), 1);
    other.close_all();
    released(&sessions).await
}
