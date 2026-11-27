use trogon_sandbox_contract::{
    ExecutionId, KillCommandRequest, KillCommandResponse, LeaseId, ListProcessesResponse, PollCommandResponse,
    UnixSignal,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{FileEnding, TestResult, exited, fixture, mount_file, mount_script, pid_and_exit};
use crate::exec::constants::{KILL_SCRIPT, PROCESS_IDENTITY};
use crate::exec::deadline::Deadline;
use crate::exec::e2b_exec::E2bExec;
use crate::exec::execution::Execution;
use crate::snapshots::{MAX_EXECUTIONS, ProcessIdentity};

pub(super) async fn insert(exec: &E2bExec, lease: &str, id: &str, deadline: Deadline) {
    let lease = LeaseId::new(lease);
    let id = ExecutionId::new(id);
    let execution = Execution {
        lease_id: lease.clone(),
        dir: Execution::directory(&id),
        user: "user".to_owned(),
        max_output_bytes: 1024,
        identity: ProcessIdentity { pid: 123, started: 456 },
        deadline,
    };
    exec.executions.lock().await.insert((lease, id), execution);
}

pub(super) async fn mount_failing_file(server: &MockServer, suffix: &'static str) {
    Mock::given(method("GET"))
        .and(path("/files"))
        .and(FileEnding(suffix))
        .respond_with(ResponseTemplate::new(500))
        .mount(server)
        .await;
}

fn kill_request(id: &str) -> KillCommandRequest {
    KillCommandRequest {
        lease_id: LeaseId::new("lease-1"),
        execution_id: ExecutionId::new(id),
        signal: UnixSignal::Kill,
    }
}

#[tokio::test]
async fn a_poll_reports_each_way_a_command_ends_or_fails() -> TestResult {
    let server = MockServer::start().await;
    let (exec, sandbox) = fixture(&server)?;
    insert(&exec, "lease-1", "settled", Deadline::Terminal(5)).await;
    insert(&exec, "lease-1", "live", Deadline::Lease).await;

    mount_file(&server, "/out", Some("o")).await;
    mount_file(&server, "/err", Some("e")).await;
    let PollCommandResponse::Completed(output) = exec.handle_poll_command(&sandbox, "lease-1", "settled").await else {
        return Err("a settled command is completed".into());
    };
    assert_eq!(
        (output.exit_code, output.stdout, output.stderr),
        (5, b"o".to_vec(), b"e".to_vec())
    );

    server.reset().await;
    mount_failing_file(&server, "/out").await;
    mount_file(&server, "/err", Some("e")).await;
    assert!(matches!(
        exec.handle_poll_command(&sandbox, "lease-1", "settled").await,
        PollCommandResponse::Denied(_)
    ));

    server.reset().await;
    mount_file(&server, "/out", Some("o")).await;
    mount_failing_file(&server, "/err").await;
    assert!(matches!(
        exec.handle_poll_command(&sandbox, "lease-1", "settled").await,
        PollCommandResponse::Denied(_)
    ));

    server.reset().await;
    mount_file(&server, "/exit", Some("0")).await;
    mount_script(&server, PROCESS_IDENTITY, exited(1)?).await;
    assert!(matches!(
        exec.handle_poll_command(&sandbox, "lease-1", "live").await,
        PollCommandResponse::Denied(_)
    ));

    server.reset().await;
    mount_failing_file(&server, "/exit").await;
    assert!(matches!(
        exec.handle_poll_command(&sandbox, "lease-1", "live").await,
        PollCommandResponse::Denied(_)
    ));
    Ok(())
}

#[tokio::test]
async fn a_kill_that_cannot_confirm_the_process_is_denied_or_settled() -> TestResult {
    let server = MockServer::start().await;
    let (exec, sandbox) = fixture(&server)?;
    insert(&exec, "lease-1", "live", Deadline::Lease).await;
    let request = kill_request("live");

    mount_failing_file(&server, "/exit").await;
    assert!(matches!(
        exec.handle_kill_command(&sandbox, "lease-1", &request).await,
        KillCommandResponse::Denied(_)
    ));

    server.reset().await;
    mount_file(&server, "/exit", Some("0")).await;
    mount_script(&server, PROCESS_IDENTITY, exited(1)?).await;
    assert!(matches!(
        exec.handle_kill_command(&sandbox, "lease-1", &request).await,
        KillCommandResponse::Denied(_)
    ));

    server.reset().await;
    mount_script(&server, KILL_SCRIPT, exited(1)?).await;
    mount_script(&server, PROCESS_IDENTITY, pid_and_exit()?).await;
    mount_file(&server, "/exit", None).await;
    assert!(matches!(
        exec.handle_kill_command(&sandbox, "lease-1", &request).await,
        KillCommandResponse::Denied(_)
    ));

    server.reset().await;
    mount_script(&server, KILL_SCRIPT, exited(1)?).await;
    mount_script(&server, PROCESS_IDENTITY, exited(44)?).await;
    Mock::given(method("GET"))
        .and(path("/files"))
        .and(FileEnding("/exit"))
        .respond_with(ResponseTemplate::new(404))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_file(&server, "/exit", Some("137")).await;
    assert_eq!(
        exec.handle_kill_command(&sandbox, "lease-1", &request).await,
        KillCommandResponse::AlreadyCompleted
    );
    Ok(())
}

#[tokio::test]
async fn the_execution_inventory_is_bounded_and_scoped_to_its_lease() -> TestResult {
    let server = MockServer::start().await;
    let (exec, sandbox) = fixture(&server)?;
    insert(&exec, "lease-2", "elsewhere", Deadline::Lease).await;
    insert(&exec, "lease-1", "mine", Deadline::Lease).await;
    let captured = exec.capture_executions("lease-1").await?;
    assert_eq!(captured.len(), 1);

    assert!(exec.restore_executions(&sandbox, "lease-1", &[]).await.is_err());

    server.reset().await;
    mount_script(&server, PROCESS_IDENTITY, exited(44)?).await;
    mount_file(&server, "/exit", None).await;
    assert!(matches!(
        exec.handle_list_processes(&sandbox, "lease-1").await,
        ListProcessesResponse::Denied(_)
    ));

    for index in 0..MAX_EXECUTIONS {
        insert(&exec, "lease-1", &format!("extra-{index:04}"), Deadline::Lease).await;
    }
    assert!(exec.capture_executions("lease-1").await.is_err());
    assert!(matches!(
        exec.handle_list_processes(&sandbox, "lease-1").await,
        ListProcessesResponse::Denied(_)
    ));
    Ok(())
}
