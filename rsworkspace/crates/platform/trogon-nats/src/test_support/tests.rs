use std::process::Command;

use testcontainers_modules::testcontainers::{
    bollard::{body_full, errors::Error as DockerError, query_parameters::UploadToContainerOptionsBuilder},
    core::client::docker_client_instance,
};

use super::*;

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

async fn assert_removed(id: &str) {
    assert!(id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let docker = docker_client_instance().await.unwrap();
    let result = docker.inspect_container(id, None).await;
    assert!(matches!(
        result,
        Err(DockerError::DockerResponseServerError { status_code: 404, .. })
    ));
}
