use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_contract::{BootMode, PrivilegedAccess};
use trogon_sandbox_providers::{
    CommitSha, EgressPolicy, GitProvider, ImageRef, NoopArtifactSink, RepoRef, SandboxError, SandboxHandle,
    SandboxProvider, SandboxSpec, SandboxStatus, ToolSelector, ToolSpec,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn sample_spec() -> Result<SandboxSpec, Box<dyn std::error::Error>> {
    Ok(SandboxSpec {
        image: ImageRef::new("sandbox-agent:dev"),
        cpu: 1.0,
        memory_mib: 512,
        timeout: Duration::from_secs(60),
        repo: RepoRef {
            provider: GitProvider::GitHub,
            owner: "acme".to_string(),
            name: "widget".to_string(),
        },
        resolved_sha: CommitSha::new("a".repeat(40))?,
        base_sha: None,
        tools: vec![ToolSpec {
            selector: ToolSelector("diff".to_string()),
            args: vec![],
        }],
        artifact_sink: Arc::new(NoopArtifactSink),
        egress_policy: EgressPolicy::DenyAll,
        network_policy: None,
        clone: None,
        host_mount: None,
        extra_binds: vec![],
        boot_mode: BootMode::default(),
        working_dir: None,
        user: None,
        privileged: PrivilegedAccess::Denied,
        exposed_ports: vec![],
        env: Default::default(),
        lease_id: None,
        disk_mib: None,
    })
}

struct StubHandle;

#[async_trait::async_trait]
impl SandboxHandle for StubHandle {
    fn id(&self) -> &str {
        "stub-1"
    }

    async fn status(&self) -> SandboxStatus {
        SandboxStatus::Running
    }

    async fn cancel(&self) -> Result<(), SandboxError> {
        Ok(())
    }
}

struct StubProvider;

#[async_trait::async_trait]
impl SandboxProvider for StubProvider {
    async fn launch(&self, _spec: SandboxSpec) -> Result<Box<dyn SandboxHandle>, SandboxError> {
        Ok(Box::new(StubHandle))
    }
}

#[tokio::test]
async fn provider_launches_and_handle_reports_status() -> TestResult {
    let provider = StubProvider;
    let handle = provider.launch(sample_spec()?).await?;
    assert_eq!(handle.id(), "stub-1");
    assert_eq!(handle.status().await, SandboxStatus::Running);
    handle.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn handle_defaults_publish_nothing() -> TestResult {
    let handle = StubProvider.launch(sample_spec()?).await?;
    assert!(handle.policy_file_path().is_none());
    assert!(handle.sidecar_id().is_none());
    assert!(handle.network_policy_name().is_none());
    assert!(handle.config_map_name().is_none());
    assert!(handle.port_endpoints().is_empty());
    Ok(())
}
