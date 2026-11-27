//! Named long-running processes: idempotent same-name start, listing, and
//! readiness probes.
//!
//! ```text
//! cargo test -p trogon-sandbox-docker-worker named_processes
//! ```

mod docker_support;

use docker_support::{TestResult, build_provider, fresh_lease_id, lease_request};

use std::num::NonZeroU16;
use std::time::Duration;

use trogon_sandbox_contract::{
    BootMode, CommandDeadline, CommandEnv, ExecutionId, LeaseId, LeaseRequest, ListProcessesResponse, OutputRetention,
    ProcessName, ProcessSpec, ProcessState, ReadinessNeedle, ReadinessProbe, StdinMode, StreamedOutput,
    WaitReadyResponse,
};
use trogon_sandbox_docker_worker::adapter::DockerWorkerProvider;
use trogon_sandbox_protocol::Provider;

async fn start(
    provider: &DockerWorkerProvider,
    lease_id: &LeaseId,
    script: &str,
    process: ProcessSpec,
) -> TestResult<ExecutionId> {
    Ok(provider
        .run_command_for_lease_detached(
            lease_id,
            vec!["sh".to_string(), "-c".to_string(), script.to_string()],
            None,
            &CommandEnv::default(),
            60,
            None,
            StreamedOutput::Disabled,
            StdinMode::Closed,
            OutputRetention::Buffer,
            CommandDeadline::Timeout,
            usize::MAX,
            Some(process),
        )
        .await?)
}

fn bare_lease_request() -> TestResult<LeaseRequest> {
    lease_request("named-processes-test", BootMode::Bare)
}

#[tokio::test(flavor = "multi_thread")]
async fn same_name_start_returns_the_running_execution_and_lists_it() -> TestResult {
    let provider = build_provider("named_processes").await?;
    let lease_id = fresh_lease_id();
    provider.lease(&bare_lease_request()?, &lease_id).await?;

    let spec = ProcessSpec {
        name: Some(ProcessName::new("sleeper")?),
        readiness: None,
    };
    let first = start(&provider, &lease_id, "sleep 30", spec.clone()).await?;
    let second = start(&provider, &lease_id, "sleep 30", spec).await?;
    assert_eq!(first, second, "a repeated name must not start a duplicate");

    let ListProcessesResponse::Processes { processes } = provider.list_processes_for_lease(&lease_id).await else {
        return Err("expected a process list".into());
    };
    assert_eq!(processes.len(), 1);
    assert_eq!(processes[0].name.as_ref().map(ProcessName::as_str), Some("sleeper"));
    assert_eq!(processes[0].state, ProcessState::Running);

    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn output_probe_becomes_ready_and_exit_before_ready_fails() -> TestResult {
    let provider = build_provider("named_processes").await?;
    let lease_id = fresh_lease_id();
    provider.lease(&bare_lease_request()?, &lease_id).await?;

    let ready = start(
        &provider,
        &lease_id,
        "sleep 1; echo server-is-up; sleep 30",
        ProcessSpec {
            name: Some(ProcessName::new("server")?),
            readiness: Some(ReadinessProbe::OutputContains {
                needle: ReadinessNeedle::new("server-is-up")?,
            }),
        },
    )
    .await?;
    assert_eq!(
        provider
            .wait_ready_for_lease(&lease_id, &ready, Duration::from_secs(10))
            .await,
        WaitReadyResponse::Ready
    );

    let doomed = start(
        &provider,
        &lease_id,
        "exit 3",
        ProcessSpec {
            name: None,
            readiness: Some(ReadinessProbe::OutputContains {
                needle: ReadinessNeedle::new("never")?,
            }),
        },
    )
    .await?;
    assert_eq!(
        provider
            .wait_ready_for_lease(&lease_id, &doomed, Duration::from_secs(10))
            .await,
        WaitReadyResponse::Failed { exit_code: 3 }
    );

    let quiet = start(
        &provider,
        &lease_id,
        "sleep 30",
        ProcessSpec {
            name: None,
            readiness: Some(ReadinessProbe::OutputContains {
                needle: ReadinessNeedle::new("never")?,
            }),
        },
    )
    .await?;
    assert_eq!(
        provider
            .wait_ready_for_lease(&lease_id, &quiet, Duration::from_secs(1))
            .await,
        WaitReadyResponse::NotReady
    );

    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn tcp_probe_becomes_ready_once_the_port_listens() -> TestResult {
    let provider = build_provider("named_processes").await?;
    let lease_id = fresh_lease_id();
    provider.lease(&bare_lease_request()?, &lease_id).await?;

    let id = start(
        &provider,
        &lease_id,
        "sleep 1; nc -l -p 8099",
        ProcessSpec {
            name: Some(ProcessName::new("listener")?),
            readiness: Some(ReadinessProbe::TcpPort {
                port: NonZeroU16::new(8099).ok_or("port")?,
            }),
        },
    )
    .await?;
    assert_eq!(
        provider
            .wait_ready_for_lease(&lease_id, &id, Duration::from_secs(15))
            .await,
        WaitReadyResponse::Ready
    );

    provider.cancel(&lease_id).await?;
    Ok(())
}
