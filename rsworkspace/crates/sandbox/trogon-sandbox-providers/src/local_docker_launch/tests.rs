use std::collections::BTreeMap;
use std::error::Error;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use trogon_sandbox_contract::{BootMode, HostMountAccess, PrivilegedAccess};

use super::{boot_overrides, create_ca_file, workspace_binds};
use crate::{
    CloneCredentials, CommitSha, DockerError, EgressPolicy, GitProvider, HostMount, ImageRef, NoopArtifactSink,
    RepoRef, SandboxSpec,
};

fn spec() -> SandboxSpec {
    SandboxSpec {
        image: ImageRef::new("img"),
        cpu: 1.0,
        memory_mib: 64,
        timeout: Duration::from_secs(1),
        repo: RepoRef {
            provider: GitProvider::GitHub,
            owner: "o".to_string(),
            name: "r".to_string(),
        },
        resolved_sha: CommitSha::placeholder(),
        base_sha: None,
        tools: vec![],
        artifact_sink: Arc::new(NoopArtifactSink),
        egress_policy: EgressPolicy::DenyAll,
        network_policy: None,
        clone: None,
        host_mount: None,
        extra_binds: vec!["/x:/y".to_string()],
        disk_mib: None,
        boot_mode: BootMode::default(),
        working_dir: None,
        user: None,
        privileged: PrivilegedAccess::Denied,
        exposed_ports: vec![],
        env: BTreeMap::new(),
        lease_id: None,
    }
}

#[test]
fn bare_boot_sleeps_and_default_boot_keeps_the_image() {
    assert_eq!(
        boot_overrides(BootMode::Bare),
        (Some(vec![]), Some(vec!["sleep".to_string(), "infinity".to_string()]))
    );
    assert_eq!(boot_overrides(BootMode::EntrypointDefault), (None, None));
}

#[test]
fn no_workspace_keeps_only_the_extra_binds() -> Result<(), DockerError> {
    assert_eq!(workspace_binds(&spec(), "/ws")?, vec!["/x:/y".to_string()]);
    Ok(())
}

#[test]
fn clone_mounts_the_workspace_read_only() -> Result<(), DockerError> {
    let mut spec = spec();
    spec.clone = Some(CloneCredentials {
        clone_url: "https://h/r".to_string(),
        auth: trogon_sandbox_contract::CloneAuth::None(trogon_sandbox_contract::EmptyObject {}),
    });
    assert_eq!(
        workspace_binds(&spec, "/ws")?,
        vec![format!("/ws:{}:ro", crate::WORKSPACE_MOUNT_PATH), "/x:/y".to_string()]
    );
    Ok(())
}

#[test]
fn host_mount_follows_its_access() -> Result<(), DockerError> {
    let mut spec = spec();
    spec.host_mount = Some(HostMount {
        path: PathBuf::from("/h"),
        access: HostMountAccess::ReadOnly,
    });
    assert_eq!(
        workspace_binds(&spec, "/ws")?[0],
        format!("/h:{}:ro", crate::WORKSPACE_MOUNT_PATH)
    );
    spec.host_mount = Some(HostMount {
        path: PathBuf::from("/h"),
        access: HostMountAccess::ReadWrite,
    });
    assert_eq!(
        workspace_binds(&spec, "/ws")?[0],
        format!("/h:{}", crate::WORKSPACE_MOUNT_PATH)
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn non_utf8_host_mount_is_rejected() {
    let mut spec = spec();
    spec.host_mount = Some(HostMount {
        path: PathBuf::from(std::ffi::OsStr::from_bytes(b"/h/\xff")),
        access: HostMountAccess::ReadWrite,
    });
    assert!(matches!(
        workspace_binds(&spec, "/ws"),
        Err(DockerError::NonUtf8Path { what: "host_mount", .. })
    ));
}

#[tokio::test]
async fn ca_file_is_created_empty_or_fails() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let path = create_ca_file(dir.path()).await?;
    assert_eq!(std::fs::read(path)?, Vec::<u8>::new());
    let result = create_ca_file(&dir.path().join("absent")).await;
    assert!(matches!(result, Err(DockerError::EgressCaFile { .. })));
    Ok(())
}
