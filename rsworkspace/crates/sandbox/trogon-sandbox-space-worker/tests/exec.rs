mod support;

use std::error::Error;

use serde_json::json;
use support::{
    ATTACH_TOKEN, Plan, Reply, Rig, TestResult, closed_origin, exit_body, frame, hanging_events, run_request,
};
use trogon_sandbox_contract::{CommandOutput, LeaseError, LeaseId, RunCommandRequest, RunCommandResponse};

async fn run(rig: &Rig, lease: &LeaseId, plan: Plan) -> Result<RunCommandResponse, Box<dyn Error>> {
    rig.space.state().next_plans.push_back(plan);
    Ok(rig.provider.run_command(&run_request(lease, &["echo", "hi"], 5)?).await)
}

fn completed(response: RunCommandResponse) -> Result<CommandOutput, Box<dyn Error>> {
    match response {
        RunCommandResponse::Completed(output) => Ok(output),
        other => Err(format!("not completed: {other:?}").into()),
    }
}

fn denied(response: RunCommandResponse) -> String {
    match response {
        RunCommandResponse::Denied(LeaseError::Backend { message }) => message,
        RunCommandResponse::Denied(LeaseError::InvalidSpec { reason }) => reason,
        _ => String::new(),
    }
}

fn body(frames: &[&str]) -> Plan {
    Plan::Events(vec![Reply::Body(frames.concat())])
}

#[tokio::test]
async fn collects_output_and_the_exit_code() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let reply = Reply::Body(exit_body(b"out", b"err", 7, "exited"));
    let output = completed(run(&rig, &lease, Plan::Events(vec![reply])).await?)?;
    assert_eq!(output.stdout, b"out");
    assert_eq!(output.stderr, b"err");
    assert_eq!(output.exit_code, 7);
    let state = rig.space.state();
    let (_, last_id, auth) = state.event_requests.first().ok_or("no events request")?;
    assert_eq!(auth, &format!("Bearer {ATTACH_TOKEN}"));
    assert!(last_id.is_none());
    let command = state.commands.last().ok_or("no command")?;
    assert_eq!(command["timeout"], "5s");
    Ok(())
}

#[tokio::test]
async fn passes_the_environment_and_working_directory() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let request: RunCommandRequest = serde_json::from_value(json!({
        "lease_id": lease, "command": ["pwd"], "cwd": "/work", "env": {"A": "1"}, "timeout_seconds": 5,
    }))?;
    completed(rig.provider.run_command(&request).await)?;
    let state = rig.space.state();
    let command = state.commands.last().ok_or("no command")?;
    assert_eq!(command["cwd"], "/work");
    assert_eq!(command["env"]["A"], "1");
    Ok(())
}

#[tokio::test]
async fn a_launch_key_makes_the_command_key_stable() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let request: RunCommandRequest = serde_json::from_value(json!({
        "lease_id": lease, "command": ["true"], "launch_key": "launch-1",
    }))?;
    completed(rig.provider.run_command(&request).await)?;
    Ok(())
}

#[tokio::test]
async fn a_timed_out_command_reports_it() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let reply = Reply::Body(exit_body(b"", b"", 0, "timed_out"));
    let output = completed(run(&rig, &lease, Plan::Events(vec![reply])).await?)?;
    assert!(output.timed_out.is_timed_out());
    Ok(())
}

#[tokio::test]
async fn caps_the_output_it_keeps() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    rig.space
        .state()
        .next_plans
        .push_back(Plan::Events(vec![Reply::Body(exit_body(
            &[b'x'; 64],
            b"",
            0,
            "exited",
        ))]));
    let request: RunCommandRequest = serde_json::from_value(json!({
        "lease_id": lease, "command": ["yes"], "max_output_bytes": 8,
    }))?;
    let output = completed(rig.provider.run_command(&request).await)?;
    assert!(output.truncated.is_truncated());
    assert!(output.stdout.len() < 64);
    Ok(())
}

#[tokio::test]
async fn resumes_a_dropped_stream_after_the_last_event() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let first = frame(1, &json!({"type": "stdout", "data_base64": "aGVsbG8="}));
    let second = frame(
        2,
        &json!({"type": "process_exited", "exit_code": 0, "status": "exited"}),
    );
    let output = completed(
        run(
            &rig,
            &lease,
            Plan::Events(vec![Reply::Body(first), Reply::Body(second)]),
        )
        .await?,
    )?;
    assert_eq!(output.stdout, b"hello");
    let state = rig.space.state();
    assert_eq!(state.event_requests.len(), 2);
    assert_eq!(state.event_requests[1].1.as_deref(), Some("1"));
    Ok(())
}

#[tokio::test]
async fn a_stream_that_keeps_ending_gives_up() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let message = denied(run(&rig, &lease, body(&[""])).await?);
    assert!(message.contains("ended before the exit event"));
    assert_eq!(rig.space.state().event_requests.len(), 4);
    Ok(())
}

#[tokio::test]
async fn a_stream_cut_mid_event_before_any_id_gives_up() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let message = denied(run(&rig, &lease, body(&["data: {\"type\""])).await?);
    assert!(message.contains("ended before the exit event"));
    assert_eq!(rig.space.state().event_requests.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_refused_stream_connection_is_retried_then_unstarted() -> TestResult {
    let rig = Rig::split(&closed_origin().await?).await?;
    let lease = rig.lease("lease-1").await?;
    let response = rig.provider.run_command(&run_request(&lease, &["true"], 5)?).await;
    assert!(denied(response).contains("did not run the command"));
    Ok(())
}

#[tokio::test]
async fn a_stream_that_will_not_open_is_unknown() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let plan = Plan::Events(vec![Reply::Status(500)]);
    assert!(denied(run(&rig, &lease, plan).await?).contains("outcome is unknown"));
    Ok(())
}

#[tokio::test]
async fn a_rejected_stream_is_unstarted() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let plan = Plan::Events(vec![Reply::Status(401)]);
    assert!(denied(run(&rig, &lease, plan).await?).contains("did not run the command"));
    Ok(())
}

#[tokio::test]
async fn a_failure_after_attaching_is_unknown_even_if_refused_before() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let first = frame(1, &json!({"type": "stdout", "data_base64": "aGk="}));
    let plan = Plan::Events(vec![Reply::Body(first), Reply::Status(401)]);
    assert!(denied(run(&rig, &lease, plan).await?).contains("outcome is unknown"));
    Ok(())
}

#[tokio::test]
async fn a_stream_that_never_answers_runs_into_the_deadline() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let request = run_request(&lease, &["sleep", "9"], 1)?;
    rig.space.state().next_plans.push_back(Plan::Events(vec![Reply::Stall]));
    let message = denied(rig.provider.run_command(&request).await);
    assert!(message.contains("did not open before the deadline"));
    Ok(())
}

#[tokio::test]
async fn a_silent_stream_is_killed_at_the_deadline() -> TestResult {
    let data = hanging_events(": keepalive\n\n").await?;
    let rig = Rig::split(&data).await?;
    let lease = rig.lease("lease-1").await?;
    let request = run_request(&lease, &["sleep", "9"], 1)?;
    rig.space
        .state()
        .next_plans
        .push_back(Plan::EventsAt(format!("{data}/events/x")));
    let message = denied(rig.provider.run_command(&request).await);
    assert!(message.contains("kill accepted"));
    Ok(())
}

#[tokio::test]
async fn a_failed_kill_is_reported() -> TestResult {
    let data = hanging_events(": keepalive\n\n").await?;
    let rig = Rig::split(&data).await?;
    let lease = rig.lease("lease-1").await?;
    let request = run_request(&lease, &["sleep", "9"], 1)?;
    {
        let mut state = rig.space.state();
        state.kill_status = Some(400);
        state.next_plans.push_back(Plan::EventsAt(format!("{data}/events/x")));
    }
    let message = denied(rig.provider.run_command(&request).await);
    assert!(message.contains("termination failed"));
    Ok(())
}

#[tokio::test]
async fn refuses_a_stream_on_another_origin() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let plan = Plan::EventsAt("http://127.0.0.1:1/events/x".into());
    let message = denied(run(&rig, &lease, plan).await?);
    assert!(message.contains("origin"));
    assert!(rig.space.state().event_requests.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_command_without_a_stream_did_not_start() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    for plan in [Plan::NoAttach, Plan::NoEventsUrl] {
        assert!(denied(run(&rig, &lease, plan).await?).contains("no event stream"));
    }
    Ok(())
}

#[tokio::test]
async fn a_refused_command_did_not_start_and_an_unavailable_one_is_unknown() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    assert!(denied(run(&rig, &lease, Plan::CreateStatus(400)).await?).contains("did not run the command"));
    assert!(denied(run(&rig, &lease, Plan::CreateStatus(500)).await?).contains("outcome is unknown"));
    assert_eq!(rig.space.state().commands.len(), 2);
    Ok(())
}

#[tokio::test]
async fn reports_frames_that_lose_output() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    for kind in ["resync_required", "error"] {
        let plan = body(&[&frame(1, &json!({"type": kind}))]);
        assert!(denied(run(&rig, &lease, plan).await?).contains(kind));
    }
    Ok(())
}

#[tokio::test]
async fn reports_output_that_is_not_base64() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let plan = body(&[&frame(1, &json!({"type": "stdout", "data_base64": "!!"}))]);
    assert!(denied(run(&rig, &lease, plan).await?).contains("not base64"));
    Ok(())
}

#[tokio::test]
async fn reports_an_exit_without_a_code() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let plan = body(&[&frame(1, &json!({"type": "process_exited"}))]);
    assert!(denied(run(&rig, &lease, plan).await?).contains("no exit code"));
    Ok(())
}

#[tokio::test]
async fn an_exit_without_a_status_or_with_an_oversized_code_still_completes() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let plan = body(&[&frame(1, &json!({"type": "process_exited", "exit_code": 1_i64 << 40}))]);
    assert_eq!(completed(run(&rig, &lease, plan).await?)?.exit_code, -1);
    Ok(())
}

#[tokio::test]
async fn ignores_frames_it_does_not_know() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let plan = body(&[
        &frame(1, &json!({"type": "heartbeat"})),
        &frame(2, &json!({"type": "stdout"})),
        &frame(
            3,
            &json!({"type": "process_exited", "exit_code": 0, "status": "exited"}),
        ),
    ]);
    assert_eq!(completed(run(&rig, &lease, plan).await?)?.exit_code, 0);
    Ok(())
}

#[tokio::test]
async fn reports_a_stream_that_is_not_json_an_object_or_too_large() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    for payload in ["data: nope\n\n", "data: 5\n\n"] {
        assert!(denied(run(&rig, &lease, body(&[payload])).await?).contains("outcome is unknown"));
    }
    let huge = format!("data: {}\n\n", "x".repeat(3 * 1024 * 1024));
    assert!(denied(run(&rig, &lease, body(&[&huge])).await?).contains("outcome is unknown"));
    Ok(())
}

#[tokio::test]
async fn refuses_options_it_does_not_wire() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    let cases = [
        (json!({"command": []}), "command must not be empty"),
        (json!({"command": ["x"], "timeout_seconds": 0}), "timeout"),
        (json!({"command": ["x"], "timeout_seconds": 3601}), "timeout"),
        (json!({"command": ["x"], "detached": true}), "detached"),
        (json!({"command": ["x"], "stream_output": true}), "stream"),
        (json!({"command": ["x"], "run_as_user": "root"}), "user"),
        (json!({"command": ["x"], "require_launch_cancellation": true}), "cancel"),
        (json!({"command": ["x"], "cwd": "relative"}), "working directory"),
        (json!({"command": ["x"], "cwd": "/a\u{0}b"}), "working directory"),
    ];
    for (mut extra, needle) in cases {
        extra["lease_id"] = json!(lease);
        let request: RunCommandRequest = serde_json::from_value(extra)?;
        let message = denied(rig.provider.run_command(&request).await);
        assert!(message.contains(needle), "{message} should mention {needle}");
    }
    assert!(rig.space.state().commands.is_empty());
    Ok(())
}

#[tokio::test]
async fn refuses_a_lease_it_does_not_hold() -> TestResult {
    let rig = Rig::start().await?;
    let response = rig
        .provider
        .run_command(&run_request(&LeaseId::new("nobody"), &["true"], 5)?)
        .await;
    assert!(matches!(
        response,
        RunCommandResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    Ok(())
}
