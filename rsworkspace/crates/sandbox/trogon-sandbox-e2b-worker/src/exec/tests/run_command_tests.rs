use std::time::Duration;

use trogon_sandbox_contract::{
    CommandExecution, LaunchCancellation, LaunchKey, LeaseError, ProviderKind, RunCommandRequest, RunCommandResponse,
    StdinMode,
};
use trogon_sandbox_protocol::subjects;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::command_scope_tests::signed;
use super::control_tests::mount_failing_file;
use super::controlled_tests::{Controlled, controlled, controlled_request};
use super::{Script, TestResult, detached, exited, mount_file, mount_script, pid_and_exit, run_request};
use crate::exec::command_phase::CommandPhase;
use crate::exec::constants::{LAUNCH_SCRIPT, PROCESS_IDENTITY, REMOVE_DIRECTORY_SCRIPT};

impl Controlled {
    async fn run_scoped(&self, request: &RunCommandRequest) -> Result<RunCommandResponse, Box<dyn std::error::Error>> {
        let subject = subjects::provider_run_command(ProviderKind::E2b, &request.lease_id);
        let (scope, payload) = signed(request, &request.lease_id, &subject)?;
        Ok(self
            .exec
            .handle_run_command_with_scope(&self.sandbox, request, &scope, &payload)
            .await)
    }
}

fn keyed(key: &str, execution: CommandExecution) -> Result<RunCommandRequest, Box<dyn std::error::Error>> {
    Ok(RunCommandRequest {
        launch_key: Some(LaunchKey::new(key)?),
        execution,
        ..run_request(&["sleep", "60"])
    })
}

fn detached_execution() -> CommandExecution {
    CommandExecution::Detached {
        stdin: StdinMode::Closed,
        deadline: Default::default(),
        process: None,
    }
}

fn denied(response: &RunCommandResponse) -> bool {
    matches!(response, RunCommandResponse::Denied(LeaseError::InvalidSpec { .. }))
}

#[tokio::test]
async fn a_scoped_run_applies_custody_only_to_keyed_or_cancellable_launches() -> TestResult {
    let c = controlled().await?;
    c.guest().await?;

    let unkeyed = detached(&["sleep", "1"]);
    assert!(matches!(c.run_scoped(&unkeyed).await?, RunCommandResponse::Started(_)));

    let keyed_detached = keyed("ordinary", detached_execution())?;
    assert!(matches!(
        c.run_scoped(&keyed_detached).await?,
        RunCommandResponse::Started(_)
    ));
    c.seed("retained", CommandPhase::Running, false, true)?;
    let retained = keyed("retained", detached_execution())?;
    assert!(denied(&c.run_scoped(&retained).await?));

    let empty = RunCommandRequest {
        command: vec![],
        require_launch_cancellation: LaunchCancellation::Required,
        ..keyed("empty", detached_execution())?
    };
    assert!(denied(&c.run_scoped(&empty).await?));
    let attached = RunCommandRequest {
        require_launch_cancellation: LaunchCancellation::Required,
        ..keyed("attached", CommandExecution::Attached)?
    };
    assert!(denied(&c.run_scoped(&attached).await?));
    let unkeyed_cancellable = RunCommandRequest {
        launch_key: None,
        require_launch_cancellation: LaunchCancellation::Required,
        ..detached(&["x"])
    };
    assert!(denied(&c.run_scoped(&unkeyed_cancellable).await?));

    let request = controlled_request("cancellable")?;
    assert!(matches!(c.run_scoped(&request).await?, RunCommandResponse::Started(_)));

    let subject = subjects::provider_run_command(ProviderKind::E2b, &request.lease_id);
    let (scope, payload) = signed(&request, &request.lease_id, &subject)?;
    let other = controlled_request("another")?;
    let response = c
        .exec
        .handle_run_command_with_scope(&c.sandbox, &other, &scope, &payload)
        .await;
    assert!(denied(&response));
    Ok(())
}

#[tokio::test]
async fn a_foreground_command_needs_a_settled_native_end_and_output() -> TestResult {
    let server = MockServer::start().await;
    let (exec, sandbox) = super::fixture(&server)?;
    let request = run_request(&["x"]);
    mount_script(&server, LAUNCH_SCRIPT, pid_and_exit()?).await;
    mount_script(&server, PROCESS_IDENTITY, exited(44)?).await;
    mount_file(&server, "/exit", Some("0")).await;
    mount_failing_file(&server, "/out").await;
    assert!(matches!(
        exec.handle_run_command(&sandbox, &request).await,
        RunCommandResponse::Denied(_)
    ));

    server.reset().await;
    mount_script(&server, LAUNCH_SCRIPT, pid_and_exit()?).await;
    mount_script(&server, PROCESS_IDENTITY, exited(44)?).await;
    Mock::given(method("GET"))
        .and(path("/files"))
        .and(super::FileEnding("/exit"))
        .respond_with(ResponseTemplate::new(200).set_body_string("0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_file(&server, "/exit", None).await;
    assert!(matches!(
        exec.handle_run_command(&sandbox, &request).await,
        RunCommandResponse::Denied(_)
    ));
    Ok(())
}

#[tokio::test]
async fn a_foreground_command_whose_lease_is_forgotten_is_denied() -> TestResult {
    let server = MockServer::start().await;
    let (exec, sandbox) = super::fixture(&server)?;
    mount_script(&server, LAUNCH_SCRIPT, pid_and_exit()?).await;
    mount_script(&server, PROCESS_IDENTITY, pid_and_exit()?).await;
    mount_file(&server, "/exit", None).await;
    let request = run_request(&["x"]);
    let forget = async {
        while exec.executions.lock().await.is_empty() {
            tokio::task::yield_now().await;
        }
        exec.forget_lease("lease-1").await;
    };
    let (response, ()) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(exec.handle_run_command(&sandbox, &request), forget)
    })
    .await?;
    assert!(matches!(response, RunCommandResponse::Denied(_)));
    Ok(())
}

#[tokio::test]
async fn a_foreground_command_forgotten_while_it_is_cleaned_up_still_completes() -> TestResult {
    let server = MockServer::start().await;
    let (exec, sandbox) = super::fixture(&server)?;
    mount_script(&server, LAUNCH_SCRIPT, pid_and_exit()?).await;
    mount_script(&server, PROCESS_IDENTITY, exited(44)?).await;
    mount_file(&server, "/exit", Some("0")).await;
    mount_file(&server, "/out", Some("o")).await;
    mount_file(&server, "/err", Some("e")).await;
    Mock::given(method("POST"))
        .and(path("/process.Process/Start"))
        .and(Script(REMOVE_DIRECTORY_SCRIPT))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(exited(0)?)
                .set_delay(Duration::from_millis(500)),
        )
        .mount(&server)
        .await;
    let request = run_request(&["x"]);
    let forget = async {
        loop {
            let cleaning = server
                .received_requests()
                .await
                .unwrap_or_default()
                .iter()
                .any(|received| {
                    received
                        .body
                        .get(5..)
                        .and_then(|body| serde_json::from_slice::<serde_json::Value>(body).ok())
                        .is_some_and(|body| body["process"]["args"][6] == REMOVE_DIRECTORY_SCRIPT)
                });
            if cleaning {
                break;
            }
            tokio::task::yield_now().await;
        }
        exec.forget_lease("lease-1").await;
    };
    let (response, ()) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(exec.handle_run_command(&sandbox, &request), forget)
    })
    .await?;
    assert!(matches!(response, RunCommandResponse::Completed(_)));
    Ok(())
}
