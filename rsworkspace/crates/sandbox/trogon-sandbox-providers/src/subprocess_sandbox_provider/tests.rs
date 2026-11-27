use std::ffi::OsString;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use trogon_sandbox_contract::{BootMode, CloneAuth, HostMountAccess, PrivilegedAccess};

use super::*;
use crate::{
    CloneCredentials, CommitSha, EgressPolicy, GitProvider, HostMount, ImageRef, NoopArtifactSink, RepoRef,
    SandboxStatus, SpawnedProcess, ToolSelector, ToolSpec,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn spec() -> Result<SandboxSpec, Box<dyn std::error::Error>> {
    Ok(SandboxSpec {
        image: ImageRef::new("img"),
        cpu: 1.0,
        memory_mib: 256,
        timeout: Duration::from_secs(60),
        repo: RepoRef {
            provider: GitProvider::GitHub,
            owner: "acme".to_string(),
            name: "widget".to_string(),
        },
        resolved_sha: CommitSha::new("a".repeat(40))?,
        base_sha: None,
        tools: vec![
            ToolSpec {
                selector: ToolSelector("diff".to_string()),
                args: vec![],
            },
            ToolSpec {
                selector: ToolSelector("lint".to_string()),
                args: vec![],
            },
        ],
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

type SpawnRecord = (String, Vec<(String, String)>);

struct RecordingSpawner {
    seen: StdMutex<Vec<SpawnRecord>>,
}

#[async_trait::async_trait]
impl ProcessSpawner for RecordingSpawner {
    async fn spawn(&self, binary: &str, env: Vec<(String, String)>) -> Result<SpawnedProcess, SubprocessError> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push((binary.to_string(), env));
        }
        let child = tokio::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .map_err(SubprocessError::Spawn)?;
        Ok(SpawnedProcess {
            id: "fake-1".to_string(),
            child: Arc::new(tokio::sync::Mutex::new(Some(child))),
        })
    }
}

struct FailingSpawner;

#[async_trait::async_trait]
impl ProcessSpawner for FailingSpawner {
    async fn spawn(&self, _binary: &str, _env: Vec<(String, String)>) -> Result<SpawnedProcess, SubprocessError> {
        Err(SubprocessError::Spawn(std::io::Error::other("no")))
    }
}

fn recording() -> Arc<RecordingSpawner> {
    Arc::new(RecordingSpawner {
        seen: StdMutex::new(Vec::new()),
    })
}

#[tokio::test]
async fn a_launch_spawns_the_agent_with_the_sandbox_environment() -> TestResult {
    let spawner = recording();
    let provider = SubprocessSandboxProvider::with_spawner(spawner.clone(), "agent-bin", "/work");
    let mut spec = spec()?;
    spec.base_sha = Some(CommitSha::new("b".repeat(40))?);
    spec.env.insert("SANDBOX_REPO".to_string(), "overridden".to_string());
    spec.env.insert("CUSTOM".to_string(), "x".to_string());
    let handle = provider.launch(spec).await?;
    assert_eq!(handle.id(), "fake-1");

    let seen = spawner.seen.lock().map_err(|_| "poisoned")?;
    let (binary, env) = seen.first().ok_or("spawned")?;
    assert_eq!(binary, "agent-bin");
    let sha = "a".repeat(40);
    let workspace = format!("/work/widget-{sha}");
    let value = |name: &str| {
        env.iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(value("CUSTOM"), Some("x"));
    assert_eq!(value("SANDBOX_REPO"), Some("widget"));
    assert_eq!(value("SANDBOX_OWNER"), Some("acme"));
    assert_eq!(value("SANDBOX_PROVIDER"), Some("github"));
    assert_eq!(value("SANDBOX_RESOLVED_SHA"), Some(sha.as_str()));
    assert_eq!(value("SANDBOX_WORKSPACE"), Some(workspace.as_str()));
    assert_eq!(
        value("SANDBOX_EVIDENCE_ROOT"),
        Some(format!("{workspace}/evidence").as_str())
    );
    assert_eq!(value("SANDBOX_TOOLS"), Some("diff,lint"));
    assert_eq!(value("SANDBOX_BASE_SHA"), Some("b".repeat(40).as_str()));
    Ok(())
}

#[tokio::test]
async fn a_launch_without_a_base_sha_omits_it() -> TestResult {
    let spawner = recording();
    let provider = SubprocessSandboxProvider::with_spawner(spawner.clone(), "agent-bin", "/work");
    let handle = provider.launch(spec()?).await?;
    assert_ne!(handle.status().await, SandboxStatus::Cancelled);
    let seen = spawner.seen.lock().map_err(|_| "poisoned")?;
    let (_, env) = seen.first().ok_or("spawned")?;
    assert!(env.iter().all(|(key, _)| key != "SANDBOX_BASE_SHA"));
    Ok(())
}

#[tokio::test]
async fn a_host_mount_is_rejected() -> TestResult {
    let provider = SubprocessSandboxProvider::with_spawner(recording(), "agent-bin", "/work");
    let mut spec = spec()?;
    spec.host_mount = Some(HostMount {
        path: "/srv".into(),
        access: HostMountAccess::ReadOnly,
    });
    let error = provider.launch(spec).await.err();
    assert!(matches!(
        error,
        Some(SandboxError::Subprocess(SubprocessError::HostMountUnsupported))
    ));
    Ok(())
}

#[tokio::test]
async fn a_spawn_failure_is_reported() -> TestResult {
    let provider = SubprocessSandboxProvider::with_spawner(Arc::new(FailingSpawner), "agent-bin", "/work");
    let error = provider.launch(spec()?).await.err();
    assert!(matches!(
        error,
        Some(SandboxError::Subprocess(SubprocessError::Spawn(_)))
    ));
    Ok(())
}

#[tokio::test]
async fn a_clone_runs_before_the_agent_is_spawned() -> TestResult {
    let dir = tempfile::tempdir()?;
    let script = dir.path().join("git.sh");
    std::fs::write(&script, "echo \"$@\" >> \"$0.log\"\n")?;
    let spawner = recording();
    let provider = SubprocessSandboxProvider::with_spawner(spawner.clone(), "agent-bin", "/work")
        .with_git(GitCommand::new("sh", [OsString::from(&script)]));
    let mut spec = spec()?;
    spec.clone = Some(CloneCredentials {
        clone_url: "https://h/r.git".to_string(),
        auth: CloneAuth::None(Default::default()),
    });
    let result = provider.launch(spec).await;
    #[cfg(feature = "volume-cache")]
    assert!(matches!(
        result.err(),
        Some(SandboxError::Subprocess(SubprocessError::VolumeCache(_)))
    ));
    #[cfg(not(feature = "volume-cache"))]
    {
        result?;
        assert!(std::fs::read_to_string(dir.path().join("git.sh.log"))?.contains("clone"));
    }
    Ok(())
}

#[test]
fn the_default_constructor_uses_the_real_spawner() {
    let provider = SubprocessSandboxProvider::new("agent-bin", "/work");
    assert_eq!(provider.agent_binary, "agent-bin");
}
