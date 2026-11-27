use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_contract::{BootMode, CloneAuth, PrivilegedAccess};

use super::*;
use crate::{CommitSha, EgressPolicy, GitProvider, ImageRef, NoopArtifactSink, RepoRef};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const URL: &str = "https://github.com/acme/widget.git";

pub(super) fn spec(clone: bool) -> Result<SandboxSpec, Box<dyn std::error::Error>> {
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
        clone: clone.then(|| CloneCredentials {
            clone_url: URL.to_string(),
            auth: CloneAuth::None(Default::default()),
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

#[test]
fn clone_credentials_follow_the_clone_field() -> TestResult {
    assert!(clone_credentials(&spec(false)?).is_none());
    let with = spec(true)?;
    assert_eq!(clone_credentials(&with).map(|c| c.clone_url.as_str()), Some(URL));
    Ok(())
}

#[cfg(not(feature = "volume-cache"))]
#[tokio::test]
async fn without_the_cache_the_clone_just_runs() -> TestResult {
    let result: Result<(), VolumeCacheError> =
        try_clone_with_volume_cache(&spec(true)?, std::path::Path::new("/unused"), async { Ok(()) }).await;
    assert!(result.is_ok());
    Ok(())
}
