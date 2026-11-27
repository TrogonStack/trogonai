use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_contract::{BootMode, CloneAuth, EmptyObject, PrivilegedAccess, SecretRef};

use super::{clone_token, clone_workspace_git, inject_token_into_url, run_program};
use crate::{
    CloneCredentials, CommitSha, DockerError, EgressPolicy, GitProvider, ImageRef, NoopArtifactSink, RepoRef,
    SandboxSpec,
};

fn spec(sha: &str) -> Result<SandboxSpec, Box<dyn Error>> {
    Ok(SandboxSpec {
        image: ImageRef::new("img"),
        cpu: 1.0,
        memory_mib: 64,
        timeout: Duration::from_secs(1),
        repo: RepoRef {
            provider: GitProvider::GitHub,
            owner: "o".to_string(),
            name: "r".to_string(),
        },
        resolved_sha: CommitSha::new(sha.to_string())?,
        base_sha: None,
        tools: vec![],
        artifact_sink: Arc::new(NoopArtifactSink),
        egress_policy: EgressPolicy::DenyAll,
        network_policy: None,
        clone: None,
        host_mount: None,
        extra_binds: vec![],
        disk_mib: None,
        boot_mode: BootMode::default(),
        working_dir: None,
        user: None,
        privileged: PrivilegedAccess::Denied,
        exposed_ports: vec![],
        env: BTreeMap::new(),
        lease_id: None,
    })
}

fn git(dir: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// A one-commit repository, built with plumbing so no commit hook or
/// identity configuration is involved. Returns the commit sha.
fn source_repository(dir: &Path) -> Result<String, Box<dyn Error>> {
    git(dir, &["init", "-q"])?;
    std::fs::write(dir.join("file.txt"), "hello")?;
    git(dir, &["add", "."])?;
    let tree = git(dir, &["write-tree"])?;
    let sha = git(dir, &["commit-tree", "-m", "init", &tree])?;
    git(dir, &["update-ref", "HEAD", &sha])?;
    Ok(sha)
}

fn literal(value: &str) -> SecretRef {
    SecretRef::Literal {
        value: value.to_string(),
    }
}

fn nats_kv() -> SecretRef {
    SecretRef::NatsKv {
        bucket: "b".to_string(),
        key: "k".to_string(),
    }
}

#[test]
fn token_is_injected_after_the_scheme() -> Result<(), DockerError> {
    assert_eq!(
        inject_token_into_url("https://github.com/o/r.git", "tok")?,
        "https://oauth2:tok@github.com/o/r.git"
    );
    Ok(())
}

#[test]
fn url_without_a_scheme_is_rejected() {
    assert!(matches!(
        inject_token_into_url("github.com/o/r.git", "tok"),
        Err(DockerError::CloneUrlMissingScheme)
    ));
}

#[test]
fn clone_token_reads_literal_secrets_only() -> Result<(), DockerError> {
    assert_eq!(clone_token(&CloneAuth::None(EmptyObject {}))?, None);
    assert_eq!(
        clone_token(&CloneAuth::BearerToken {
            token_ref: literal("a")
        })?,
        Some("a")
    );
    assert_eq!(
        clone_token(&CloneAuth::Basic {
            username: "u".to_string(),
            password_ref: literal("b")
        })?,
        Some("b")
    );
    assert!(matches!(
        clone_token(&CloneAuth::BearerToken { token_ref: nats_kv() }),
        Err(DockerError::NatsKvSecretUnsupported)
    ));
    assert!(matches!(
        clone_token(&CloneAuth::Basic {
            username: "u".to_string(),
            password_ref: nats_kv()
        }),
        Err(DockerError::NatsKvSecretUnsupported)
    ));
    Ok(())
}

#[tokio::test]
async fn missing_program_is_a_spawn_error() {
    let result = run_program("sandbox-no-such-program", "clone", &[]).await;
    assert!(matches!(result, Err(DockerError::GitSpawn { command: "clone", .. })));
}

#[tokio::test]
async fn failing_program_reports_its_stderr() {
    let result = run_program("git", "checkout", &["--no-such-flag"]).await;
    assert!(matches!(
        result,
        Err(DockerError::GitFailed {
            command: "checkout",
            ..
        })
    ));
}

#[tokio::test]
async fn succeeding_program_is_ok() {
    assert!(run_program("git", "version", &["--version"]).await.is_ok());
}

#[tokio::test]
async fn git_clone_checks_out_the_resolved_commit() -> Result<(), Box<dyn Error>> {
    let source = tempfile::tempdir()?;
    let sha = source_repository(source.path())?;
    let target = tempfile::tempdir()?;
    let workspace = target.path().join("ws");
    let creds = CloneCredentials {
        clone_url: format!("file://{}", source.path().display()),
        auth: CloneAuth::None(EmptyObject {}),
    };
    let workspace_path = workspace.to_string_lossy().into_owned();
    clone_workspace_git(&spec(&sha)?, &creds, &workspace_path).await?;
    assert_eq!(std::fs::read_to_string(workspace.join("file.txt"))?, "hello");
    Ok(())
}

#[tokio::test]
async fn git_clone_fails_when_the_commit_cannot_be_fetched() -> Result<(), Box<dyn Error>> {
    let source = tempfile::tempdir()?;
    source_repository(source.path())?;
    let target = tempfile::tempdir()?;
    let workspace_path = target.path().join("ws").to_string_lossy().into_owned();
    let creds = CloneCredentials {
        clone_url: format!("file://{}", source.path().display()),
        auth: CloneAuth::None(EmptyObject {}),
    };
    let missing = "0123456789abcdef0123456789abcdef01234567";
    let result = clone_workspace_git(&spec(missing)?, &creds, &workspace_path).await;
    assert!(matches!(result, Err(DockerError::GitFailed { command: "fetch", .. })));
    Ok(())
}

#[tokio::test]
async fn git_clone_injects_the_literal_token_into_the_url() -> Result<(), Box<dyn Error>> {
    let target = tempfile::tempdir()?;
    let workspace_path = target.path().join("ws").to_string_lossy().into_owned();
    let creds = CloneCredentials {
        clone_url: "no-scheme".to_string(),
        auth: CloneAuth::BearerToken {
            token_ref: literal("t"),
        },
    };
    let result = clone_workspace_git(&spec(&"a".repeat(40))?, &creds, &workspace_path).await;
    assert!(matches!(result, Err(DockerError::CloneUrlMissingScheme)));
    let creds = CloneCredentials {
        clone_url: "file:///nonexistent/sandbox-clone-source".to_string(),
        auth: CloneAuth::BearerToken {
            token_ref: literal("t"),
        },
    };
    let result = clone_workspace_git(&spec(&"a".repeat(40))?, &creds, &workspace_path).await;
    assert!(matches!(result, Err(DockerError::GitFailed { command: "clone", .. })));
    Ok(())
}
