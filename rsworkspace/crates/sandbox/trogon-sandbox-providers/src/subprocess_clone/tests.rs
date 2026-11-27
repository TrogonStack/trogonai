use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_contract::{BootMode, PrivilegedAccess};

use super::*;
use crate::{CloneCredentials, CommitSha, EgressPolicy, GitProvider, ImageRef, NoopArtifactSink, RepoRef};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SCRIPT: &str = "echo \"$@\" >> \"$0.log\"\ncase \" $* \" in\n  *\" $(cat \"$0.fail\" 2>/dev/null) \"*) echo boom >&2; exit 1;;\nesac\n";

fn spec(url: &str, auth: CloneAuth) -> Result<SandboxSpec, Box<dyn std::error::Error>> {
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
        tools: vec![],
        artifact_sink: Arc::new(NoopArtifactSink),
        egress_policy: EgressPolicy::DenyAll,
        network_policy: None,
        clone: Some(CloneCredentials {
            clone_url: url.to_string(),
            auth,
        }),
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

fn bearer(value: &str) -> CloneAuth {
    CloneAuth::BearerToken {
        token_ref: SecretRef::Literal {
            value: value.to_string(),
        },
    }
}

struct FakeGit {
    dir: tempfile::TempDir,
    git: GitCommand,
}

impl FakeGit {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let script = dir.path().join("git.sh");
        std::fs::write(&script, SCRIPT)?;
        let git = GitCommand::new("sh", [OsString::from(script)]);
        Ok(Self { dir, git })
    }

    fn fail_on(&self, word: &str) -> Result<(), std::io::Error> {
        std::fs::write(self.dir.path().join("git.sh.fail"), word)
    }

    fn log(&self) -> Result<String, std::io::Error> {
        std::fs::read_to_string(self.dir.path().join("git.sh.log"))
    }
}

#[tokio::test]
async fn a_spec_without_a_clone_runs_no_git() -> TestResult {
    let fake = FakeGit::new()?;
    let mut spec = spec("https://h/r.git", CloneAuth::None(Default::default()))?;
    spec.clone = None;
    clone_workspace_subprocess_git(&fake.git, &spec, "/ws").await?;
    assert!(fake.log().is_err());
    Ok(())
}

#[tokio::test]
async fn an_anonymous_clone_runs_clone_fetch_and_checkout() -> TestResult {
    let fake = FakeGit::new()?;
    let spec = spec("https://h/r.git", CloneAuth::None(Default::default()))?;
    clone_workspace_subprocess_git(&fake.git, &spec, "/ws").await?;
    let sha = "a".repeat(40);
    assert_eq!(
        fake.log()?,
        format!(
            "clone --depth=1 --filter=blob:none https://h/r.git /ws\n-C /ws fetch --depth=1 origin {sha}\n-C /ws checkout {sha}\n"
        )
    );
    Ok(())
}

#[tokio::test]
async fn a_token_is_embedded_in_the_clone_url() -> TestResult {
    let fake = FakeGit::new()?;
    clone_workspace_subprocess_git(&fake.git, &spec("https://h/r.git", bearer("tok"))?, "/ws").await?;
    assert!(
        fake.log()?
            .starts_with("clone --depth=1 --filter=blob:none https://oauth2:tok@h/r.git /ws\n")
    );

    let basic = CloneAuth::Basic {
        username: "user".to_string(),
        password_ref: SecretRef::Literal {
            value: "pw".to_string(),
        },
    };
    let fake = FakeGit::new()?;
    clone_workspace_subprocess_git(&fake.git, &spec("https://h/r.git", basic)?, "/ws").await?;
    assert!(fake.log()?.contains("https://oauth2:pw@h/r.git"));
    Ok(())
}

#[tokio::test]
async fn a_url_without_a_scheme_cannot_carry_a_token() -> TestResult {
    let fake = FakeGit::new()?;
    let result = clone_workspace_subprocess_git(&fake.git, &spec("h/r.git", bearer("tok"))?, "/ws").await;
    assert!(matches!(result, Err(SubprocessError::CloneUrlMissingScheme)));
    Ok(())
}

#[tokio::test]
async fn nats_kv_secrets_are_unsupported() -> TestResult {
    let kv = || SecretRef::NatsKv {
        bucket: "b".to_string(),
        key: "k".to_string(),
    };
    let fake = FakeGit::new()?;
    let bearer = CloneAuth::BearerToken { token_ref: kv() };
    let basic = CloneAuth::Basic {
        username: "u".to_string(),
        password_ref: kv(),
    };
    for auth in [bearer, basic] {
        let result = clone_workspace_subprocess_git(&fake.git, &spec("https://h/r.git", auth)?, "/ws").await;
        assert!(matches!(result, Err(SubprocessError::NatsKvSecretUnsupported)));
    }
    Ok(())
}

#[tokio::test]
async fn each_git_step_failure_names_its_step() -> TestResult {
    for (word, step) in [
        ("clone", GitStep::Clone),
        ("fetch", GitStep::Fetch),
        ("checkout", GitStep::Checkout),
    ] {
        let fake = FakeGit::new()?;
        fake.fail_on(word)?;
        let spec = spec("https://h/r.git", CloneAuth::None(Default::default()))?;
        let result = clone_workspace_subprocess_git(&fake.git, &spec, "/ws").await;
        assert!(
            matches!(&result, Err(SubprocessError::GitFailed { step: failed, stderr }) if *failed == step && stderr.contains("boom")),
            "{word}: {:?}",
            result.map(|_| ())
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_missing_git_program_is_unavailable() -> TestResult {
    let git = GitCommand::new("/definitely/not/git", []);
    let spec = spec("https://h/r.git", CloneAuth::None(Default::default()))?;
    let result = clone_workspace_subprocess_git(&git, &spec, "/ws").await;
    assert!(matches!(
        result,
        Err(SubprocessError::GitUnavailable {
            step: GitStep::Clone,
            ..
        })
    ));
    Ok(())
}

#[tokio::test]
async fn the_cached_entry_point_clones_through_git_without_a_cache() -> TestResult {
    let fake = FakeGit::new()?;
    let spec = spec("https://h/r.git", CloneAuth::None(Default::default()))?;
    let result = clone_workspace_subprocess(&fake.git, &spec, "/ws").await;
    #[cfg(feature = "volume-cache")]
    assert!(matches!(
        result,
        Err(SubprocessError::VolumeCache(crate::VolumeCacheError::BucketUnset))
    ));
    #[cfg(not(feature = "volume-cache"))]
    {
        result?;
        assert!(fake.log()?.contains("checkout"));
    }
    let _ = Path::new("");
    Ok(())
}
