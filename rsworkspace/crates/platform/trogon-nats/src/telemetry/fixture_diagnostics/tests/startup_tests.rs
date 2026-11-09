use std::panic::AssertUnwindSafe;
use std::process::Command;

use super::*;
use futures::FutureExt;
use testcontainers_modules::testcontainers::{
    bollard::errors::Error as DockerError, bollard::models::NetworkDisconnectRequest,
    bollard::query_parameters::RemoveContainerOptionsBuilder, core::WaitFor, core::error::ExecError,
};
use trogon_std::env::{ReadEnv, SystemEnv};

const CHILD_MODE: &str = "NATS_DIAGNOSTICS_CONTRACT_CHILD";

// Private access is required to replace readiness and cancel the shared startup
// owner. The public fixture intentionally provides neither failure injection.
#[tokio::test]
async fn shared_runner_timeout_retains_evidence_before_removal() {
    if SystemEnv.var(CHILD_MODE).as_deref() == Ok("timeout") {
        let request = plain().with_ready_conditions(vec![WaitFor::message_on_stderr("never-emitted-marker")]);
        let diagnostics = FixtureDiagnostics::for_request(&request).unwrap();
        diagnostics.start(request).await;
        panic!("unsatisfied readiness unexpectedly returned a container");
    }
    let output = child("shared_runner_timeout_retains_evidence_before_removal", "timeout");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("\"startup_result\":\"startup_timeout\""), "{stderr}");
    assert!(stderr.contains("Server is ready"), "{stderr}");
    assert!(stderr.contains("\"kind\":\"container_state\""), "{stderr}");
    assert!(stderr.contains("connection_not_attempted"));
    assert!(!stderr.contains("never-emitted-marker"));
    assert_removed(&stderr).await;
}

#[tokio::test]
async fn cancelling_shared_startup_retains_evidence_and_removes_the_owned_container() {
    if SystemEnv.var(CHILD_MODE).as_deref() == Ok("cancel") {
        let request = plain().with_ready_conditions(vec![WaitFor::message_on_stderr("never-emitted-marker")]);
        let diagnostics = FixtureDiagnostics::for_request(&request).unwrap();
        tokio::select! {
            _ = diagnostics.start(request) => panic!("unsatisfied readiness unexpectedly returned a container"),
            () = async {
                loop {
                    let report = diagnostics.report();
                    if report.contains("Server is ready") && report.contains("owned-container=") {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            } => {}
        }
        return;
    }
    let output = child(
        "cancelling_shared_startup_retains_evidence_and_removes_the_owned_container",
        "cancel",
    );
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("startup_future_did_not_return_container"), "{stderr}");
    assert!(stderr.contains("Server is ready"), "{stderr}");
    assert!(stderr.contains("connection_not_attempted"));
    assert!(!stderr.contains("startup_result"));
    assert_removed(&stderr).await;
}

#[tokio::test]
async fn readiness_failure_is_classified_without_sdk_error_detail() {
    if SystemEnv.var(CHILD_MODE).as_deref() == Ok("readiness") {
        let request = plain().with_ready_conditions(vec![WaitFor::healthcheck()]);
        let diagnostics = FixtureDiagnostics::for_request(&request).unwrap();
        diagnostics.start(request).await;
        panic!("missing healthcheck unexpectedly returned a container");
    }
    let output = child("readiness_failure_is_classified_without_sdk_error_detail", "readiness");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("\"startup_result\":\"readiness_error\""), "{stderr}");
    assert!(!stderr.contains("healthcheck is not configured"));
    assert!(stderr.contains("connection_not_attempted"));
}

#[tokio::test]
async fn docker_start_failure_suppresses_caller_controlled_error_context() {
    if SystemEnv.var(CHILD_MODE).as_deref() == Ok("docker") {
        let request = plain().with_user("sensitive-fixture-marker");
        let diagnostics = FixtureDiagnostics::for_request(&request).unwrap();
        let failure = AssertUnwindSafe(diagnostics.start(request))
            .catch_unwind()
            .await
            .unwrap_err();
        // The SDK has not constructed its drop-owned handle when Docker start fails.
        let docker = docker_client_instance().await.unwrap();
        let inspected = docker.inspect_container(&diagnostics.name, None).await.unwrap();
        let config = inspected.config.unwrap();
        assert_eq!(config.image.as_deref(), Some(diagnostics.image));
        assert_eq!(config.labels.unwrap().get(constants::LABEL), Some(&diagnostics.name));
        assert_eq!(
            inspected.name.as_deref(),
            Some(format!("/{}", diagnostics.name).as_str())
        );
        let id = inspected.id.unwrap();
        assert!(is_hex(&id, 64));
        docker
            .remove_container(&id, Some(RemoveContainerOptionsBuilder::new().force(true).build()))
            .await
            .unwrap();
        eprintln!("owned-container={id}");
        std::panic::resume_unwind(failure);
    }
    let output = child(
        "docker_start_failure_suppresses_caller_controlled_error_context",
        "docker",
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("\"startup_result\":\"docker_client_error\""),
        "{stderr}"
    );
    assert!(!stderr.contains("sensitive-fixture-marker"));
    assert!(stderr.contains("connection_not_attempted"));
    assert_removed(&stderr).await;
}

#[tokio::test]
async fn sdk_error_categories_retain_only_the_static_startup_contract() {
    let container = plain().with_network("bridge").start().await.unwrap();
    let port_error = container.get_host_port_ipv4(64221).await.unwrap_err();
    docker_client_instance()
        .await
        .unwrap()
        .disconnect_network(
            "bridge",
            NetworkDisconnectRequest {
                container: container.id().to_owned(),
                force: Some(true),
            },
        )
        .await
        .unwrap();
    let missing_info = container.get_bridge_ip_address().await.unwrap_err();
    assert!(matches!(port_error, TestcontainersError::PortNotExposed { .. }));
    assert!(matches!(missing_info, TestcontainersError::MissingInfo(_)));
    for (error, expected) in [
        (port_error, "port_not_exposed"),
        (missing_info, "missing_container_info"),
        (
            TestcontainersError::Exec(ExecError::ExitCodeMismatch { expected: 0, actual: 7 }),
            "exec_error",
        ),
        (
            TestcontainersError::Io(std::io::Error::other("sensitive-fixture-marker")),
            "io_error",
        ),
        (TestcontainersError::other("sensitive-fixture-marker"), "other_error"),
    ] {
        let diagnostics = FixtureDiagnostics::for_request(&plain()).unwrap();
        diagnostics.record(FixtureObservation::StartupResult {
            startup_result: startup_error(&error),
            client_phase: "connection_not_attempted",
        });
        let report = diagnostics.report();
        assert!(report.contains(&format!("\"startup_result\":\"{expected}\"")));
        assert!(report.contains("connection_not_attempted"));
        assert!(!report.contains("sensitive-fixture-marker"));
        assert!(!report.contains(container.id()));
        assert!(!report.contains("NetworkSettings"));
        assert!(!report.contains("64221"));
    }
}

fn child(test: &str, mode: &str) -> std::process::Output {
    let output = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg(format!("telemetry::fixture_diagnostics::tests::startup_tests::{test}"))
        .arg("--nocapture")
        .env(CHILD_MODE, mode)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("running 1 test"));
    output
}

async fn assert_removed(stderr: &str) {
    let id = stderr
        .lines()
        .find_map(|line| line.strip_prefix("owned-container="))
        .expect("retained owned ID");
    assert!(is_hex(id, 64));
    let result = docker_client_instance()
        .await
        .unwrap()
        .inspect_container(id, None)
        .await;
    assert!(matches!(
        result,
        Err(DockerError::DockerResponseServerError { status_code: 404, .. })
    ));
}
