use super::*;
use crate::config::{BaseRootfs, FirecrackerBinary, KernelImage, StateDir};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn version_is_read_from_the_first_line() {
    assert_eq!(parse_vmm_version("Firecracker v1.9.0\n\nExit"), Some("v1.9.0".into()));
    assert_eq!(parse_vmm_version("Something else"), None);
    assert_eq!(parse_vmm_version(""), None);
}

#[cfg(unix)]
fn fake_vmm(dir: &Path, version: &str) -> std::io::Result<std::path::PathBuf> {
    let path = dir.join("firecracker");
    std::fs::write(&path, format!("#!/bin/sh\necho 'Firecracker {version}'\n"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

#[cfg(unix)]
fn config(dir: &Path, version: &str, kernel: &[u8]) -> std::io::Result<FirecrackerConfig> {
    let kernel_path = dir.join("vmlinux");
    std::fs::write(&kernel_path, kernel)?;
    Ok(FirecrackerConfig::new(
        FirecrackerBinary::new(fake_vmm(dir, version)?),
        KernelImage::new(kernel_path),
        BaseRootfs::new("/r"),
        StateDir::new("/s"),
    ))
}

#[cfg(unix)]
async fn digest_of(config: &FirecrackerConfig, jailer: Option<&Path>) -> Result<EnvDigest, Box<dyn std::error::Error>> {
    Ok(environment_digest(
        &definition(config, jailer, &trogon_std::env::SystemEnv).await?,
    )?)
}

#[cfg(unix)]
#[tokio::test]
async fn digest_is_stable_and_follows_each_input() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let base = digest_of(&config(tmp.path(), "v1.9.0", b"kernel")?, None).await?;
    assert_eq!(base, digest_of(&config(tmp.path(), "v1.9.0", b"kernel")?, None).await?);

    let mut changed = vec![
        digest_of(&config(tmp.path(), "v1.10.0", b"kernel")?, None).await?,
        digest_of(&config(tmp.path(), "v1.9.0", b"other kernel")?, None).await?,
    ];
    let mut c = config(tmp.path(), "v1.9.0", b"kernel")?;
    c.boot_args.push_str(" quiet");
    changed.push(digest_of(&c, None).await?);
    let mut c = config(tmp.path(), "v1.9.0", b"kernel")?;
    c.cpu_template = Some("T2".into());
    changed.push(digest_of(&c, None).await?);
    let jailer = tmp.path().join("jailer");
    std::fs::write(&jailer, b"jailer one")?;
    let c = config(tmp.path(), "v1.9.0", b"kernel")?;
    let with_jailer = digest_of(&c, Some(&jailer)).await?;
    changed.push(with_jailer.clone());
    std::fs::write(&jailer, b"jailer two")?;
    changed.push(digest_of(&c, Some(&jailer)).await?);
    for digest in &changed {
        assert_ne!(*digest, base);
    }
    assert_ne!(changed[4], changed[5]);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn install_enforces_the_node_allowlist() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let mut c = config(tmp.path(), "v1.9.0", b"kernel")?;
    let own = install(
        &mut c,
        None,
        vec![],
        LegacySnapshotPolicy::default(),
        &trogon_std::env::SystemEnv,
    )
    .await?;
    assert_eq!(c.environment.own, Some(own.clone()));

    let other = EnvDigest::new(format!("sha256:{}", "e".repeat(64)))?;
    let mut c = config(tmp.path(), "v1.9.0", b"kernel")?;
    let error = install(
        &mut c,
        None,
        vec![other.clone()],
        LegacySnapshotPolicy::default(),
        &trogon_std::env::SystemEnv,
    )
    .await
    .err()
    .ok_or("an unapproved digest must be refused")?;
    assert!(matches!(
        error.downcast_ref::<EnvironmentError>(),
        Some(EnvironmentError::NotApproved { .. })
    ));

    let mut c = config(tmp.path(), "v1.9.0", b"kernel")?;
    install(
        &mut c,
        None,
        vec![other, own],
        LegacySnapshotPolicy::default(),
        &trogon_std::env::SystemEnv,
    )
    .await?;
    Ok(())
}
