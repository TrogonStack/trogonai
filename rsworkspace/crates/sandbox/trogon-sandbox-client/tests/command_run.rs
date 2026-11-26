mod command_support;
mod common;

use std::time::Duration;

use bytes::Bytes;
use common::{MockBus, Reply, TestResult, client_replying, lease_id};
use trogon_sandbox_client::{ClientError, CommandOptions};
use trogon_sandbox_contract::{
    CommandDeadline, CommandEnv, CommandOutput, ExecutionId, LaunchCancellation, LaunchKey, OutputRetention,
    RunCommandRequest, RunCommandResponse, RunCommandStartedResponse, StdinMode, StreamedOutput,
};

fn completed(stdout: &[u8]) -> RunCommandResponse {
    RunCommandResponse::Completed(CommandOutput {
        stdout: stdout.to_vec(),
        ..Default::default()
    })
}

fn started() -> RunCommandResponse {
    RunCommandResponse::Started(RunCommandStartedResponse {
        execution_id: ExecutionId::new("exec-1"),
        origin: Default::default(),
    })
}

fn echo() -> Vec<String> {
    vec!["echo".into(), "hello".into()]
}

#[tokio::test]
async fn run_command_returns_output_when_completed() -> TestResult {
    let (client, log) = client_replying(&completed(b"hello"))?;
    let output = client
        .run_command(&lease_id(), echo(), CommandOptions::default())
        .await?;
    assert_eq!(output.stdout, b"hello");
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.run_command.lease-1");
    let body = sent.json()?;
    assert_eq!(body["command"], serde_json::json!(["echo", "hello"]));
    assert_eq!(body["detached"], false);
    Ok(())
}

#[tokio::test]
async fn run_command_sends_the_options() -> TestResult {
    let (client, log) = client_replying(&completed(b""))?;
    let options = CommandOptions {
        cwd: Some("/work".into()),
        env: CommandEnv::new([("A".to_string(), "1".to_string())].into())?,
        timeout_seconds: 7,
        stream_output: StreamedOutput::Enabled,
        ..CommandOptions::default()
    };
    client.run_command(&lease_id(), echo(), options).await?;
    let body = log.only()?.json()?;
    assert_eq!(body["cwd"], "/work");
    assert_eq!(body["env"]["A"], "1");
    assert_eq!(body["timeout_seconds"], 7);
    assert_eq!(body["stream_output"], true);
    Ok(())
}

#[tokio::test]
async fn run_command_rejects_a_started_reply() -> TestResult {
    let (client, _) = client_replying(&started())?;
    let result = client.run_command(&lease_id(), echo(), CommandOptions::default()).await;
    assert!(matches!(result, Err(ClientError::Transport(_))));
    Ok(())
}

#[tokio::test]
async fn run_command_returns_denied_when_broker_denies() -> TestResult {
    let (client, _) = client_replying(&RunCommandResponse::Denied(command_support::not_running()))?;
    let result = client.run_command(&lease_id(), echo(), CommandOptions::default()).await;
    assert!(command_support::is_not_running(&result));
    Ok(())
}

#[tokio::test]
async fn run_command_waits_ten_seconds_past_the_command_timeout() -> TestResult {
    let (client, log) = client_replying(&completed(b""))?;
    for (timeout_seconds, wait) in [(30, 40), (0, 11)] {
        let options = CommandOptions {
            timeout_seconds,
            ..CommandOptions::default()
        };
        client.run_command(&lease_id(), echo(), options).await?;
        let sent = log.all();
        let last = sent.last().ok_or("no request sent")?;
        assert_eq!(last.timeout, Duration::from_secs(wait));
    }
    Ok(())
}

#[tokio::test]
async fn run_command_retries_a_lost_reply_under_the_same_launch_key() -> TestResult {
    let bus = MockBus::scripted(vec![Reply::Timeout, Reply::json(&completed(b"once"))?]);
    let log = bus.log();
    let output = bus
        .into_client()
        .run_command(&lease_id(), echo(), CommandOptions::default())
        .await?;
    assert_eq!(output.stdout, b"once");
    let keys = log
        .all()
        .iter()
        .map(|sent| Ok(serde_json::from_slice::<RunCommandRequest>(&sent.payload)?.launch_key))
        .collect::<Result<Vec<_>, serde_json::Error>>()?;
    assert_eq!(keys.len(), 2);
    assert!(keys[0].is_some());
    assert_eq!(keys[0], keys[1]);
    Ok(())
}

#[tokio::test]
async fn run_command_retries_once_after_a_transport_failure() -> TestResult {
    let bus = MockBus::scripted(vec![Reply::Fail, Reply::json(&completed(b"ok"))?]);
    let log = bus.log();
    let output = bus
        .into_client()
        .run_command(&lease_id(), echo(), CommandOptions::default())
        .await?;
    assert_eq!(output.stdout, b"ok");
    assert_eq!(log.len(), 2);
    Ok(())
}

#[tokio::test]
async fn run_command_gives_up_after_one_retry() -> TestResult {
    let bus = MockBus::timing_out();
    let log = bus.log();
    let result = bus
        .into_client()
        .run_command(
            &lease_id(),
            echo(),
            CommandOptions {
                timeout_seconds: 30,
                ..CommandOptions::default()
            },
        )
        .await;
    assert!(matches!(result, Err(ClientError::Timeout(wait)) if wait == Duration::from_secs(40)));
    assert_eq!(log.len(), 2);
    Ok(())
}

#[tokio::test]
async fn run_command_reports_an_undecodable_reply() -> TestResult {
    let client = MockBus::replying(Bytes::from_static(b"not json")).into_client();
    let result = client.run_command(&lease_id(), echo(), CommandOptions::default()).await;
    assert!(matches!(result, Err(ClientError::Transport(_))));
    Ok(())
}

#[tokio::test]
async fn send_run_command_keeps_a_caller_supplied_launch_key() -> TestResult {
    let (client, log) = client_replying(&completed(b""))?;
    let request = RunCommandRequest {
        lease_id: lease_id(),
        command: echo(),
        cwd: None,
        env: CommandEnv::default(),
        timeout_seconds: 5,
        stream_output: StreamedOutput::Disabled,
        run_as_user: None,
        output_retention: OutputRetention::Buffer,
        max_output_bytes: None,
        redact_values: Default::default(),
        launch_key: Some(LaunchKey::new("my-key")?),
        require_launch_cancellation: LaunchCancellation::Required,
        execution: Default::default(),
    };
    let response = client.send_run_command(request).await?;
    assert_eq!(response, completed(b""));
    let body = log.only()?.json()?;
    assert_eq!(body["launch_key"], "my-key");
    assert_eq!(body["require_launch_cancellation"], true);
    Ok(())
}

#[tokio::test]
async fn run_command_detached_returns_the_execution_id() -> TestResult {
    let (client, log) = client_replying(&started())?;
    let id = client
        .run_command_detached(
            &lease_id(),
            vec!["cat".into()],
            CommandOptions {
                output_retention: OutputRetention::StreamOnly,
                ..CommandOptions::default()
            },
            StdinMode::Interactive,
            CommandDeadline::Lease,
        )
        .await?;
    assert_eq!(id, ExecutionId::new("exec-1"));
    let sent = log.only()?;
    assert_eq!(sent.timeout, Duration::from_secs(40));
    let body = sent.json()?;
    assert_eq!(body["detached"], true);
    assert_eq!(body["stdin"], "interactive");
    assert_eq!(body["output_retention"], "stream_only");
    assert_eq!(body["deadline"], "lease");
    Ok(())
}

#[tokio::test]
async fn run_process_sends_the_process_spec() -> TestResult {
    let (client, log) = client_replying(&started())?;
    let spec = trogon_sandbox_contract::ProcessSpec {
        name: Some(trogon_sandbox_contract::ProcessName::new("web")?),
        readiness: None,
    };
    client
        .run_process(
            &lease_id(),
            vec!["serve".into()],
            CommandOptions::default(),
            StdinMode::Closed,
            CommandDeadline::Timeout,
            Some(spec),
        )
        .await?;
    assert_eq!(log.only()?.json()?["process"]["name"], "web");
    Ok(())
}

#[tokio::test]
async fn run_command_detached_rejects_a_completed_reply() -> TestResult {
    let (client, _) = client_replying(&completed(b""))?;
    let result = client
        .run_command_detached(
            &lease_id(),
            vec!["sleep".into()],
            CommandOptions::default(),
            StdinMode::Closed,
            CommandDeadline::Timeout,
        )
        .await;
    assert!(matches!(result, Err(ClientError::Transport(_))));
    Ok(())
}

#[tokio::test]
async fn run_command_detached_returns_denied_when_broker_denies() -> TestResult {
    let (client, _) = client_replying(&RunCommandResponse::Denied(command_support::not_running()))?;
    let result = client
        .run_command_detached(
            &lease_id(),
            vec!["sleep".into()],
            CommandOptions::default(),
            StdinMode::Closed,
            CommandDeadline::Timeout,
        )
        .await;
    assert!(command_support::is_not_running(&result));
    Ok(())
}

#[tokio::test]
async fn an_api_key_is_sent_as_a_bearer_token() -> TestResult {
    let bus = MockBus::replying_json(&completed(b""))?;
    let log = bus.log();
    let client = bus.into_client().with_api_key("my-secret-key".into());
    client
        .run_command(&lease_id(), echo(), CommandOptions::default())
        .await?;
    assert_eq!(log.only()?.authorization().as_deref(), Some("Bearer my-secret-key"));
    Ok(())
}

#[tokio::test]
async fn no_api_key_sends_no_authorization_header() -> TestResult {
    let (client, log) = client_replying(&completed(b""))?;
    client
        .run_command(&lease_id(), echo(), CommandOptions::default())
        .await?;
    assert_eq!(log.only()?.authorization(), None);
    Ok(())
}
