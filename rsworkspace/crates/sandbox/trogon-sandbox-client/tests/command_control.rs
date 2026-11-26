mod command_support;
mod common;

use std::time::Duration;

use command_support::{execution_id, is_not_running, not_running};
use common::{MockBus, Reply, TestResult, client_replying, lease_id};
use trogon_sandbox_client::{ClientError, DEFAULT_REQUEST_TIMEOUT};
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CommandOutput, ExecutionId, KillCommandResponse,
    LaunchKey, LeaseError, ListProcessesResponse, PollCommandResponse, ProcessInfo, ProcessName, ProcessState,
    ReadyPorts, RestartCommandResponse, StdinChunk, StdinEof, StdinSeq, StopCommandResponse, StopSignal, UnixSignal,
    WaitReadyResponse, WaitSandboxReadyResponse, WriteStdinResponse,
};

fn process(id: &str, name: &str, state: ProcessState) -> Result<ProcessInfo, Box<dyn std::error::Error>> {
    Ok(ProcessInfo {
        execution_id: ExecutionId::new(id),
        name: Some(ProcessName::new(name)?),
        readiness: None,
        state,
    })
}

#[tokio::test]
async fn poll_command_returns_each_outcome() -> TestResult {
    let output = CommandOutput {
        stdout: b"done".to_vec(),
        ..Default::default()
    };
    for response in [
        PollCommandResponse::Running,
        PollCommandResponse::Completed(output),
        PollCommandResponse::NotFound,
    ] {
        let (client, log) = client_replying(&response)?;
        let result = client.poll_command(&lease_id(), &execution_id()).await?;
        assert_eq!(result, response);
        let sent = log.only()?;
        assert_eq!(sent.subject, "sandbox.lease.poll_command.lease-1");
        assert_eq!(sent.json()?["execution_id"], "exec-1");
    }
    Ok(())
}

#[tokio::test]
async fn poll_command_returns_denied_when_broker_denies() -> TestResult {
    let (client, _) = client_replying(&PollCommandResponse::Denied(not_running()))?;
    assert!(is_not_running(&client.poll_command(&lease_id(), &execution_id()).await));
    Ok(())
}

#[tokio::test]
async fn calls_without_an_explicit_timeout_use_the_configured_default() -> TestResult {
    let result = MockBus::timing_out()
        .into_client()
        .poll_command(&lease_id(), &execution_id())
        .await;
    assert!(matches!(result, Err(ClientError::Timeout(wait)) if wait == DEFAULT_REQUEST_TIMEOUT));

    let result = MockBus::timing_out()
        .into_client()
        .with_default_timeout(Duration::from_secs(3))
        .poll_command(&lease_id(), &execution_id())
        .await;
    assert!(matches!(result, Err(ClientError::Timeout(wait)) if wait == Duration::from_secs(3)));
    Ok(())
}

#[tokio::test]
async fn list_processes_returns_the_processes_and_surfaces_denial() -> TestResult {
    let listed = ListProcessesResponse::Processes {
        processes: vec![process("exec-1", "web", ProcessState::Running)?],
    };
    let (client, _) = client_replying(&listed)?;
    assert_eq!(client.list_processes(&lease_id()).await?, listed);

    let (client, _) = client_replying(&ListProcessesResponse::Unsupported)?;
    assert_eq!(
        client.list_processes(&lease_id()).await?,
        ListProcessesResponse::Unsupported
    );

    let (client, _) = client_replying(&ListProcessesResponse::Denied(not_running()))?;
    assert!(is_not_running(&client.list_processes(&lease_id()).await));
    Ok(())
}

#[tokio::test]
async fn wait_ready_sends_the_request_and_returns_each_outcome() -> TestResult {
    for response in [
        WaitReadyResponse::Ready,
        WaitReadyResponse::NotReady,
        WaitReadyResponse::Failed { exit_code: 2 },
    ] {
        let (client, log) = client_replying(&response)?;
        assert_eq!(client.wait_ready(&lease_id(), &execution_id(), 20).await?, response);
        let sent = log.only()?;
        assert_eq!(sent.subject, "sandbox.lease.wait_ready.lease-1");
        assert_eq!(sent.timeout, Duration::from_secs(35));
        assert_eq!(sent.json()?["timeout_seconds"], 20);
    }
    let (client, log) = client_replying(&WaitReadyResponse::Ready)?;
    client.wait_ready(&lease_id(), &execution_id(), 100_000).await?;
    assert_eq!(log.only()?.timeout, Duration::from_secs(315));

    let (client, _) = client_replying(&WaitReadyResponse::Denied(not_running()))?;
    assert!(is_not_running(
        &client.wait_ready(&lease_id(), &execution_id(), 1).await
    ));
    Ok(())
}

#[tokio::test]
async fn wait_sandbox_ready_sends_the_request_and_returns_each_outcome() -> TestResult {
    for response in [
        WaitSandboxReadyResponse::Ready,
        WaitSandboxReadyResponse::NotReady {
            pending_ports: vec![8080],
        },
    ] {
        let (client, log) = client_replying(&response)?;
        let ports = ReadyPorts::new(vec![8080, 9090])?;
        assert_eq!(client.wait_sandbox_ready(&lease_id(), ports, 10).await?, response);
        let sent = log.only()?;
        assert_eq!(sent.subject, "sandbox.lease.wait_sandbox_ready.lease-1");
        assert_eq!(sent.timeout, Duration::from_secs(40));
        assert_eq!(sent.json()?["ports"], serde_json::json!([8080, 9090]));
    }
    let (client, log) = client_replying(&WaitSandboxReadyResponse::Ready)?;
    client
        .wait_sandbox_ready(&lease_id(), ReadyPorts::default(), 100_000)
        .await?;
    assert_eq!(log.only()?.timeout, Duration::from_secs(330));

    let (client, _) = client_replying(&WaitSandboxReadyResponse::Denied(not_running()))?;
    let result = client.wait_sandbox_ready(&lease_id(), ReadyPorts::default(), 1).await;
    assert!(is_not_running(&result));
    Ok(())
}

#[tokio::test]
async fn kill_command_returns_signaled_and_surfaces_denial() -> TestResult {
    let (client, log) = client_replying(&KillCommandResponse::Signaled)?;
    let result = client
        .kill_command(&lease_id(), &execution_id(), UnixSignal::Kill)
        .await?;
    assert_eq!(result, KillCommandResponse::Signaled);
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.kill_command.lease-1");
    assert_eq!(sent.json()?["signal"], "kill");

    let (client, _) = client_replying(&KillCommandResponse::Denied(not_running()))?;
    let result = client
        .kill_command(&lease_id(), &execution_id(), UnixSignal::Term)
        .await;
    assert!(is_not_running(&result));
    Ok(())
}

#[tokio::test]
async fn kill_process_by_name_signals_the_running_process() -> TestResult {
    let listed = ListProcessesResponse::Processes {
        processes: vec![
            process("exec-old", "web", ProcessState::Exited { exit_code: 0 })?,
            process("exec-other", "db", ProcessState::Running)?,
            process("exec-web", "web", ProcessState::Running)?,
        ],
    };
    let bus = MockBus::scripted(vec![
        Reply::json(&listed)?,
        Reply::json(&KillCommandResponse::Signaled)?,
    ]);
    let log = bus.log();
    let result = bus
        .into_client()
        .kill_process_by_name(&lease_id(), &ProcessName::new("web")?, UnixSignal::Term)
        .await?;
    assert_eq!(result, KillCommandResponse::Signaled);
    let sent = log.all();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].json()?["execution_id"], "exec-web");
    Ok(())
}

#[tokio::test]
async fn kill_process_by_name_reports_not_found_when_nothing_runs_under_the_name() -> TestResult {
    let listed = ListProcessesResponse::Processes {
        processes: vec![process("exec-old", "web", ProcessState::Exited { exit_code: 1 })?],
    };
    let (client, log) = client_replying(&listed)?;
    let result = client
        .kill_process_by_name(&lease_id(), &ProcessName::new("web")?, UnixSignal::Term)
        .await?;
    assert_eq!(result, KillCommandResponse::NotFound);
    assert_eq!(log.len(), 1);
    Ok(())
}

#[tokio::test]
async fn kill_process_by_name_refuses_when_processes_are_unsupported() -> TestResult {
    let (client, _) = client_replying(&ListProcessesResponse::Unsupported)?;
    let result = client
        .kill_process_by_name(&lease_id(), &ProcessName::new("web")?, UnixSignal::Term)
        .await;
    assert!(matches!(
        result,
        Err(ClientError::Denied(LeaseError::InvalidSpec { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn stop_command_returns_stopped_and_bounds_its_wait_by_the_grace() -> TestResult {
    let response = StopCommandResponse::Stopped {
        signal: StopSignal::Term,
    };
    let (client, log) = client_replying(&response)?;
    assert_eq!(
        client.stop_command(&lease_id(), &execution_id(), Some(5)).await?,
        response
    );
    let sent = log.only()?;
    assert_eq!(sent.timeout, Duration::from_secs(20));
    assert_eq!(sent.json()?["grace_seconds"], 5);

    client.stop_command(&lease_id(), &execution_id(), None).await?;
    let all = log.all();
    let last = all.last().ok_or("no request sent")?;
    assert_eq!(last.timeout, Duration::from_secs(25));
    assert_eq!(last.json()?["grace_seconds"], 10);

    client.stop_command(&lease_id(), &execution_id(), Some(100_000)).await?;
    let all = log.all();
    let last = all.last().ok_or("no request sent")?;
    assert_eq!(last.timeout, Duration::from_secs(315));
    Ok(())
}

#[tokio::test]
async fn stop_command_surfaces_denied_as_error() -> TestResult {
    let (client, _) = client_replying(&StopCommandResponse::Denied(not_running()))?;
    let result = client.stop_command(&lease_id(), &execution_id(), None).await;
    assert!(is_not_running(&result));
    Ok(())
}

#[tokio::test]
async fn restart_command_returns_the_new_execution_id() -> TestResult {
    let response = RestartCommandResponse::Restarted {
        execution_id: ExecutionId::new("exec-2"),
    };
    let (client, log) = client_replying(&response)?;
    assert_eq!(
        client.restart_command(&lease_id(), &execution_id(), None).await?,
        response
    );
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.restart_command.lease-1");
    assert_eq!(sent.timeout, Duration::from_secs(40));

    let (client, _) = client_replying(&RestartCommandResponse::Denied(not_running()))?;
    let result = client.restart_command(&lease_id(), &execution_id(), Some(1)).await;
    assert!(is_not_running(&result));
    Ok(())
}

#[tokio::test]
async fn write_stdin_sends_the_request_and_returns_the_outcome() -> TestResult {
    let response = WriteStdinResponse::Written {
        next_seq: StdinSeq::new(1),
    };
    let (client, log) = client_replying(&response)?;
    let result = client
        .write_stdin(
            &lease_id(),
            &execution_id(),
            StdinSeq::new(0),
            StdinChunk::new(b"hi".to_vec())?,
            StdinEof::Close,
        )
        .await?;
    assert_eq!(result, response);
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.write_stdin.lease-1");
    let body = sent.json()?;
    assert_eq!(body["seq"], 0);
    assert_eq!(body["eof"], true);
    Ok(())
}

#[tokio::test]
async fn write_stdin_passes_non_denied_outcomes_through() -> TestResult {
    for response in [
        WriteStdinResponse::StdinClosed,
        WriteStdinResponse::NotFound,
        WriteStdinResponse::Unsupported,
    ] {
        let (client, _) = client_replying(&response)?;
        let result = client
            .write_stdin(
                &lease_id(),
                &execution_id(),
                StdinSeq::new(3),
                StdinChunk::new(b"x".to_vec())?,
                StdinEof::Keep,
            )
            .await?;
        assert_eq!(result, response);
    }
    Ok(())
}

#[tokio::test]
async fn write_stdin_returns_denied_when_broker_denies() -> TestResult {
    let (client, _) = client_replying(&WriteStdinResponse::Denied(not_running()))?;
    let result = client
        .write_stdin(
            &lease_id(),
            &execution_id(),
            StdinSeq::new(0),
            StdinChunk::new(b"x".to_vec())?,
            StdinEof::Keep,
        )
        .await;
    assert!(is_not_running(&result));
    Ok(())
}

#[tokio::test]
async fn write_stdin_refuses_an_empty_write_that_keeps_stdin_open() -> TestResult {
    let (client, log) = client_replying(&WriteStdinResponse::StdinClosed)?;
    let result = client
        .write_stdin(
            &lease_id(),
            &execution_id(),
            StdinSeq::new(0),
            StdinChunk::default(),
            StdinEof::Keep,
        )
        .await;
    assert!(matches!(
        result,
        Err(ClientError::Denied(LeaseError::InvalidSpec { .. }))
    ));
    assert!(log.is_empty());
    Ok(())
}

#[tokio::test]
async fn write_stdin_times_out_when_no_reply() -> TestResult {
    let result = MockBus::timing_out()
        .into_client()
        .write_stdin(
            &lease_id(),
            &execution_id(),
            StdinSeq::new(0),
            StdinChunk::new(b"x".to_vec())?,
            StdinEof::Keep,
        )
        .await;
    assert!(matches!(result, Err(ClientError::Timeout(_))));
    Ok(())
}

#[tokio::test]
async fn cancel_command_launch_sends_the_key_and_surfaces_denial() -> TestResult {
    let request = CancelCommandLaunchRequest {
        lease_id: lease_id(),
        launch_key: LaunchKey::new("launch-1")?,
    };
    let response = CancelCommandLaunchResponse::Canceled { execution_id: None };
    let (client, log) = client_replying(&response)?;
    assert_eq!(client.cancel_command_launch(request.clone()).await?, response);
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.cancel_command_launch.lease-1");
    assert_eq!(sent.json()?["launch_key"], "launch-1");

    let (client, _) = client_replying(&CancelCommandLaunchResponse::Denied(not_running()))?;
    assert!(is_not_running(&client.cancel_command_launch(request).await));
    Ok(())
}
