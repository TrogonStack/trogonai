use std::process::Command;

use testcontainers_modules::testcontainers::{
    bollard::{body_full, errors::Error as DockerError, query_parameters::UploadToContainerOptionsBuilder},
    core::{WaitFor, client::docker_client_instance},
};

use super::*;
use trogon_std::env::ReadEnv;

// Fault injection needs the private, test-owned container handle. Public callers cannot
// change a fixture's filesystem or readiness conditions.
#[tokio::test]
async fn stream_create_evidence_survives_container_teardown() {
    let server = JetStreamTestServer::start().await;
    let id = server.0._container.id().to_owned();
    let files = tempfile::tempdir().unwrap();
    let directory = files.path().join("tmp/nats/jetstream/$G/streams");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("AUDIT"), []).unwrap();
    let archive = Command::new("tar")
        .args(["-cf", "-", "-C"])
        .arg(files.path())
        .arg("tmp")
        .output()
        .unwrap();
    assert!(archive.status.success());
    docker_client_instance()
        .await
        .unwrap()
        .upload_to_container(
            &id,
            Some(UploadToContainerOptionsBuilder::new().path("/").build()),
            body_full(archive.stdout.into()),
        )
        .await
        .unwrap();
    let error = server
        .jetstream()
        .await
        .create_stream(jetstream::stream::Config {
            name: "AUDIT".to_owned(),
            storage: jetstream::stream::StorageType::File,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(
        matches!(error.kind(), jetstream::context::CreateStreamErrorKind::JetStream(ref error) if error.error_code().0 == 10049)
    );

    let evidence = server.failure_diagnostics().await;
    drop(server);
    assert_removed(&id).await;
    assert!(
        evidence.contains("storage directory is not a directory"),
        "missing underlying broker warning: {evidence}"
    );
    assert!(evidence.contains("AUDIT"));
    assert!(evidence.contains(&id));
    assert!(evidence.contains("size_rw"));
    eprintln!("retained-store-evidence:\n{evidence}");
}

#[test]
fn startup_evidence_survives_timeout_cleanup() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "test_support::tests::startup_failure_child", "--nocapture"])
        .env("NATS_DIAGNOSTIC_FAILURE_CHILD", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("Server is ready"),
        "missing broker readiness before cleanup: {stderr}"
    );
    assert!(
        stderr.contains("startup_timeout"),
        "missing typed startup outcome: {stderr}"
    );
    assert!(stderr.contains("nats:2.14.2"));
    assert!(stderr.contains("architecture"));
    assert!(stderr.contains("oom_killed"));
    assert!(stderr.contains("sha256:"));
    assert!(stderr.contains("connection_not_attempted"));
    let id = stderr
        .lines()
        .find_map(|line| line.strip_prefix("owned-container="))
        .unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(assert_removed(id));
    for line in stderr.lines().filter(|line| {
        line.starts_with("nats-fixture=") || line.starts_with("owned-container=") || line.starts_with("{\"fixture\":")
    }) {
        eprintln!("{line}");
    }
}

#[tokio::test]
async fn startup_failure_child() {
    if SystemEnv.var_os("NATS_DIAGNOSTIC_FAILURE_CHILD").is_none() {
        return;
    }
    let request = Nats::default()
        .with_tag(ATOMIC_BATCH_NATS_IMAGE_TAG)
        .with_cmd(&NatsServerCmd::default().with_jetstream())
        .with_log_consumer(|_: &testcontainers_modules::testcontainers::core::logs::LogFrame| {})
        .with_ready_conditions(vec![WaitFor::message_on_stderr(
            "deliberately unobserved readiness marker",
        )]);
    // The library's existing 60-second startup timeout is unchanged.
    TestServer::start(request).await;
}

async fn assert_removed(id: &str) {
    assert!(id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let docker = docker_client_instance().await.unwrap();
    let result = docker.inspect_container(id, None).await;
    assert!(matches!(
        result,
        Err(DockerError::DockerResponseServerError { status_code: 404, .. })
    ));
}
