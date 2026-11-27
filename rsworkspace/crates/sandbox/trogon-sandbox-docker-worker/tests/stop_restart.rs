//! Graceful stop (SIGTERM, then SIGKILL after a grace period) and restart of
//! detached commands.
//!
//! ```text
//! cargo test -p trogon-sandbox-docker-worker stop_restart
//! ```

mod docker_support;

use docker_support::{TestResult, build_provider, fresh_lease_id, lease_request};

use std::time::Duration;

use trogon_sandbox_contract::{
    BootMode, CommandDeadline, CommandEnv, ExecutionId, LeaseId, LeaseRequest, ListProcessesResponse, OutputRetention,
    PollCommandResponse, ProcessName, ProcessSpec, ProcessState, ReadinessNeedle, ReadinessProbe,
    RestartCommandResponse, StdinMode, StopCommandResponse, StopSignal, StreamedOutput, WaitReadyResponse,
};
use trogon_sandbox_docker_worker::adapter::DockerWorkerProvider;
use trogon_sandbox_protocol::Provider;

async fn start(
    provider: &DockerWorkerProvider,
    lease_id: &LeaseId,
    script: &str,
    process: Option<ProcessSpec>,
) -> TestResult<ExecutionId> {
    Ok(provider
        .run_command_for_lease_detached(
            lease_id,
            vec!["sh".to_string(), "-c".to_string(), script.to_string()],
            None,
            &CommandEnv::default(),
            120,
            None,
            StreamedOutput::Disabled,
            StdinMode::Closed,
            OutputRetention::Buffer,
            CommandDeadline::Timeout,
            usize::MAX,
            process,
        )
        .await?)
}

async fn leased(provider: &DockerWorkerProvider) -> TestResult<LeaseId> {
    let lease_id = fresh_lease_id();
    provider.lease(&bare_lease_request()?, &lease_id).await?;
    Ok(lease_id)
}

fn bare_lease_request() -> TestResult<LeaseRequest> {
    lease_request("stop-restart-test", BootMode::Bare)
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_ends_a_cooperative_command_with_sigterm() -> TestResult {
    let provider = build_provider("stop_restart").await?;
    let lease_id = leased(&provider).await?;
    let execution_id = start(&provider, &lease_id, "sleep 300", None).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let response = provider
        .stop_command_for_lease(&lease_id, &execution_id, Duration::from_secs(10))
        .await;
    assert_eq!(
        response,
        StopCommandResponse::Stopped {
            signal: StopSignal::Term
        }
    );
    assert!(matches!(
        provider.poll_command_for_lease(&lease_id, &execution_id).await,
        Some(PollCommandResponse::Completed(_))
    ));

    // A second stop finds it finished.
    assert_eq!(
        provider
            .stop_command_for_lease(&lease_id, &execution_id, Duration::from_secs(1))
            .await,
        StopCommandResponse::AlreadyCompleted
    );
    assert_eq!(
        provider
            .stop_command_for_lease(
                &lease_id,
                &ExecutionId::new("no-such-execution"),
                Duration::from_secs(1)
            )
            .await,
        StopCommandResponse::NotFound
    );
    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_escalates_to_sigkill_when_sigterm_is_ignored() -> TestResult {
    let provider = build_provider("stop_restart").await?;
    let lease_id = leased(&provider).await?;
    // An ignored SIGTERM disposition is inherited by the children, so only
    // SIGKILL ends this loop.
    let execution_id = start(&provider, &lease_id, "trap '' TERM; while true; do sleep 1; done", None).await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let started = std::time::Instant::now();
    let response = provider
        .stop_command_for_lease(&lease_id, &execution_id, Duration::from_secs(1))
        .await;
    assert_eq!(
        response,
        StopCommandResponse::Stopped {
            signal: StopSignal::Kill
        }
    );
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "SIGKILL must wait out the grace period"
    );
    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_reruns_a_named_process_under_a_new_execution_id() -> TestResult {
    let provider = build_provider("stop_restart").await?;
    let lease_id = leased(&provider).await?;
    let spec = ProcessSpec {
        name: Some(ProcessName::new("server")?),
        readiness: Some(ReadinessProbe::OutputContains {
            needle: ReadinessNeedle::new("server-is-up")?,
        }),
    };
    let first = start(&provider, &lease_id, "echo server-is-up; sleep 300", Some(spec)).await?;
    assert_eq!(
        provider
            .wait_ready_for_lease(&lease_id, &first, Duration::from_secs(10))
            .await,
        WaitReadyResponse::Ready
    );

    let RestartCommandResponse::Restarted { execution_id: second } = provider
        .restart_command_for_lease(&lease_id, &first, Duration::from_secs(10))
        .await
    else {
        return Err("expected Restarted".into());
    };
    assert_ne!(first, second, "a restart gets a new execution id");

    // The new execution is probed again, from its own output.
    assert_eq!(
        provider
            .wait_ready_for_lease(&lease_id, &second, Duration::from_secs(10))
            .await,
        WaitReadyResponse::Ready
    );
    let ListProcessesResponse::Processes { processes } = provider.list_processes_for_lease(&lease_id).await else {
        return Err("expected processes".into());
    };
    let running: Vec<_> = processes
        .iter()
        .filter(|p| p.name.as_ref().map(ProcessName::as_str) == Some("server") && p.state == ProcessState::Running)
        .collect();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].execution_id, second);

    // Unknown executions have nothing to restart.
    assert_eq!(
        provider
            .restart_command_for_lease(
                &lease_id,
                &ExecutionId::new("no-such-execution"),
                Duration::from_secs(1)
            )
            .await,
        RestartCommandResponse::NotFound
    );
    provider.cancel(&lease_id).await?;
    Ok(())
}
