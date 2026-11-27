use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_sandbox_providers::{
    CommitSha, EgressPolicy, GitProvider, ImageRef, K8sMicroVmConfig, NoopArtifactSink, ProcessSpawner, RepoRef,
    SandboxProvider, SandboxSpec, SpawnedProcess, SubprocessError, SubprocessSandboxProvider,
    build_workspace_tar_secret,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct CapturingSpawner {
    env: Mutex<BTreeMap<String, String>>,
}

#[async_trait]
impl ProcessSpawner for CapturingSpawner {
    async fn spawn(&self, _: &str, env: Vec<(String, String)>) -> Result<SpawnedProcess, SubprocessError> {
        *self.env.lock().await = env.into_iter().collect();
        Ok(SpawnedProcess {
            id: "captured-process".into(),
            child: Arc::new(Mutex::new(None)),
        })
    }
}

#[tokio::test]
async fn subprocess_launch_exports_authoritative_sandbox_metadata() -> TestResult {
    let spawner = Arc::new(CapturingSpawner::default());
    let provider = SubprocessSandboxProvider::with_spawner(spawner.clone(), "sandbox-agent", "/workspaces");
    let spec = SandboxSpec {
        image: ImageRef::new("sandbox-agent:dev"),
        cpu: 1.0,
        memory_mib: 64,
        timeout: Duration::from_secs(30),
        repo: RepoRef {
            provider: GitProvider::GitHub,
            owner: "example".into(),
            name: "project".into(),
        },
        resolved_sha: CommitSha::new("a".repeat(40))?,
        base_sha: Some(CommitSha::new("b".repeat(40))?),
        tools: vec![],
        artifact_sink: Arc::new(NoopArtifactSink),
        egress_policy: EgressPolicy::DenyAll,
        network_policy: None,
        clone: None,
        host_mount: None,
        extra_binds: vec![],
        boot_mode: Default::default(),
        working_dir: None,
        user: None,
        privileged: Default::default(),
        exposed_ports: vec![],
        env: BTreeMap::from([
            ("SANDBOX_REPO".into(), "spoofed".into()),
            ("APPLICATION_SETTING".into(), "retained".into()),
        ]),
        lease_id: None,
        disk_mib: None,
    };

    provider.launch(spec).await?;

    let env = spawner.env.lock().await;
    assert_eq!(env.get("SANDBOX_PROVIDER").map(String::as_str), Some("github"));
    assert_eq!(env.get("SANDBOX_OWNER").map(String::as_str), Some("example"));
    assert_eq!(env.get("SANDBOX_REPO").map(String::as_str), Some("project"));
    assert_eq!(env.get("SANDBOX_BASE_SHA"), Some(&"b".repeat(40)));
    assert_eq!(env.get("APPLICATION_SETTING").map(String::as_str), Some("retained"));
    assert!(env.keys().all(|key| !key.starts_with("LEGACY_")));
    Ok(())
}

#[test]
fn kubernetes_defaults_use_a_sandbox_service_account() {
    assert_eq!(K8sMicroVmConfig::new("sandboxes").service_account, "sandbox-agent");
}

#[test]
fn workspace_import_secret_has_sandbox_ownership_labels() -> TestResult {
    let secret = build_workspace_tar_secret("workspace", "sandboxes", "job", b"archive");
    let labels = secret.metadata.labels.ok_or("the secret carries labels")?;

    assert_eq!(
        labels.get("app.kubernetes.io/managed-by").map(String::as_str),
        Some("sandbox-worker")
    );
    assert_eq!(labels.get("sandbox.trogon.live/job").map(String::as_str), Some("job"));
    assert!(labels.keys().all(|key| !key.starts_with("legacy/")));
    Ok(())
}
