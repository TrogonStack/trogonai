mod support;

use support::*;
use trogon_sandbox_contract::NativeSnapshotId;
use trogon_sandbox_modal_worker::{
    ModalApiError, ModalAppName, ModalSandboxApi, ModalSandboxSpec, PythonBridgeModalClient,
};

const FAKE_PYTHON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fake_python.sh");
const STUB_PYTHON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/stub_python.sh");

fn spec() -> Boxed<ModalSandboxSpec> {
    Ok(ModalSandboxSpec {
        app_name: ModalAppName::new("test-app")?,
        image: "ubuntu:24.04".to_owned(),
        image_id: Some(NativeSnapshotId::new("im-1")?),
        environment: Some("main".to_owned()),
        workdir: "/mnt/workspace".to_owned(),
        timeout_seconds: 60,
        cpu_cores: None,
        memory_mib: Some(512),
        region: Some("us-east".to_owned()),
        env: vec![("KEY".to_owned(), "value".to_owned())],
        tags: vec![("trogon.lease_id".to_owned(), "lease-1".to_owned())],
        block_network: true,
        outbound_cidr_allowlist: vec!["10.0.0.0/8".to_owned()],
        encrypted_ports: vec![8080, 9090],
    })
}

fn fake() -> Boxed<PythonBridgeModalClient> {
    Ok(PythonBridgeModalClient::new(FAKE_PYTHON)?)
}

#[tokio::test]
async fn every_operation_round_trips_through_the_bridge() -> TestResult {
    let bridge = fake()?;
    let created = bridge.create_sandbox(&spec()?).await?;
    assert_eq!(created.id.as_str(), "sb-1");
    assert!(created.is_running());
    assert!(bridge.get_sandbox(&sandbox_id("sb-1")?).await?.is_running());
    let output = bridge.exec(&sandbox_id("sb-1")?, &["echo".to_owned()]).await?;
    assert_eq!(
        (output.stdout.as_str(), output.stderr.as_str(), output.exit_code),
        ("out", "err", 0)
    );
    bridge
        .write_file(&sandbox_id("sb-1")?, "/mnt/workspace/a.txt", b"data")
        .await?;
    bridge.terminate(&sandbox_id("sb-1")?).await?;
    let tunnels = bridge.tunnels(&sandbox_id("sb-1")?).await?;
    assert_eq!(tunnels.first().map(|tunnel| tunnel.tls_port), Some(443));
    let image = bridge.snapshot_filesystem(&sandbox_id("sb-1")?, 3600).await?;
    assert_eq!(image.as_str(), "im-9");
    Ok(())
}

#[tokio::test]
async fn bridge_failures_map_to_typed_errors() -> TestResult {
    let bridge = fake()?;
    let get = |id: &'static str| {
        let bridge = &bridge;
        async move {
            bridge
                .get_sandbox(&sandbox_id(id)?)
                .await
                .map_err(Box::<dyn std::error::Error>::from)
        }
    };
    let failure = |result: Boxed<_>| -> Boxed<ModalApiError> {
        match result {
            Err(error) => error.downcast::<ModalApiError>().map(|error| *error),
            Ok(_) => Err("the call succeeded".into()),
        }
    };
    assert!(matches!(
        failure(get("sb-notfound").await)?,
        ModalApiError::SandboxNotFound(_)
    ));
    assert!(matches!(failure(get("sb-auth").await)?, ModalApiError::Auth(_)));
    assert!(matches!(failure(get("sb-invalid").await)?, ModalApiError::Modal { .. }));
    assert!(matches!(failure(get("sb-weird").await)?, ModalApiError::Modal { .. }));
    assert!(matches!(
        failure(get("sb-nodetail").await)?,
        ModalApiError::Modal { .. }
    ));
    assert!(matches!(failure(get("sb-silent").await)?, ModalApiError::NoOutput));
    assert!(matches!(failure(get("sb-garbage").await)?, ModalApiError::Json(_)));
    assert!(matches!(failure(get("sb-mismatch").await)?, ModalApiError::Json(_)));
    assert!(matches!(
        failure(get("sb-crash").await)?,
        ModalApiError::Exited { ref stderr, .. } if stderr == "the bridge fell over"
    ));
    Ok(())
}

#[tokio::test]
async fn a_missing_interpreter_is_a_spawn_error() -> TestResult {
    let bridge = PythonBridgeModalClient::new("/nonexistent/python")?;
    let error = bridge.get_sandbox(&sandbox_id("sb-1")?).await.err();
    assert!(matches!(error, Some(ModalApiError::Spawn { .. })));
    Ok(())
}

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[tokio::test]
async fn the_embedded_script_drives_the_modal_sdk() -> TestResult {
    if !python3_available() {
        return Ok(());
    }
    let bridge = PythonBridgeModalClient::new(STUB_PYTHON)?;
    assert!(bridge.create_sandbox(&spec()?).await?.is_running());
    let mut plain = spec()?;
    plain.image_id = None;
    plain.environment = None;
    plain.env.clear();
    plain.tags.clear();
    plain.region = None;
    plain.block_network = false;
    plain.outbound_cidr_allowlist.clear();
    plain.encrypted_ports.clear();
    plain.memory_mib = None;
    assert!(bridge.create_sandbox(&plain).await?.is_running());
    assert!(bridge.get_sandbox(&sandbox_id("sb-1")?).await?.is_running());
    assert!(!bridge.get_sandbox(&sandbox_id("sb-done")?).await?.is_running());
    let output = bridge.exec(&sandbox_id("sb-1")?, &["echo".to_owned()]).await?;
    assert_eq!(output.stdout, "out");
    bridge
        .write_file(&sandbox_id("sb-1")?, "/mnt/workspace/a/b.txt", b"data")
        .await?;
    bridge.write_file(&sandbox_id("sb-1")?, "top.txt", b"data").await?;
    bridge.terminate(&sandbox_id("sb-1")?).await?;
    assert_eq!(bridge.tunnels(&sandbox_id("sb-1")?).await?.len(), 2);
    assert_eq!(
        bridge.snapshot_filesystem(&sandbox_id("sb-1")?, 60).await?.as_str(),
        "im-1"
    );
    let missing = bridge.get_sandbox(&sandbox_id("sb-missing")?).await.err();
    assert!(matches!(missing, Some(ModalApiError::SandboxNotFound(_))));
    Ok(())
}
