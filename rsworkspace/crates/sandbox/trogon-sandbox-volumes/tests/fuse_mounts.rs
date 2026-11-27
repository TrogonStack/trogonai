#![cfg(unix)]

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::tempdir;
use trogon_sandbox_contract::{HostMountAccess, RemoteMountSource, S3BucketName};
use trogon_sandbox_volumes::{
    FuseMetrics, FuseMountConfig, FuseMountManager, FuseOtherUsers, FusePlatform, VolumeEndpoint, VolumeError,
    VolumeRegion, build_mount_args, parse_proc_mounts_for_fuse,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn source(prefix: &str) -> Result<RemoteMountSource, Box<dyn std::error::Error>> {
    Ok(RemoteMountSource::S3 {
        bucket: S3BucketName::new("tenant-bucket")?,
        prefix: prefix.to_owned(),
    })
}

fn strings(args: Vec<OsString>) -> Vec<String> {
    args.into_iter().map(|a| a.to_string_lossy().into_owned()).collect()
}

fn script(dir: &Path, name: &str, body: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

fn config(dir: &Path, mount_body: &str) -> Result<FuseMountConfig, Box<dyn std::error::Error>> {
    Ok(FuseMountConfig {
        mount_s3_binary: script(dir, "mount-s3", mount_body)?,
        platform: FusePlatform::Supported,
        ready_timeout: Duration::from_millis(200),
        ready_poll_interval: Duration::from_millis(10),
        mount_table: dir.join("mounts"),
        unmount_binary: script(dir, "fusermount", "exit 0")?,
        fallback_unmount_binary: script(dir, "umount", "exit 1")?,
        ..FuseMountConfig::default()
    })
}

#[test]
fn mount_args_cover_every_option() -> TestResult {
    let config = FuseMountConfig {
        region: Some(VolumeRegion::new("us-east-1")?),
        endpoint: Some(VolumeEndpoint::new("http://localhost:9000")?),
        other_users: FuseOtherUsers::Allowed,
        ..FuseMountConfig::default()
    };
    let args = strings(build_mount_args(
        &config,
        &source("prefix/path")?,
        Path::new("/mnt/data"),
        HostMountAccess::ReadOnly,
    ));
    assert_eq!(
        args,
        [
            "tenant-bucket",
            "/mnt/data",
            "--prefix",
            "prefix/path/",
            "--region",
            "us-east-1",
            "--endpoint-url",
            "http://localhost:9000",
            "--read-only",
            "--allow-other",
        ]
    );
    Ok(())
}

#[test]
fn mount_args_omit_unset_options() -> TestResult {
    let args = strings(build_mount_args(
        &FuseMountConfig::default(),
        &source("")?,
        Path::new("/mnt/data"),
        HostMountAccess::ReadWrite,
    ));
    assert_eq!(args, ["tenant-bucket", "/mnt/data"]);

    let args = strings(build_mount_args(
        &FuseMountConfig::default(),
        &source("kept/")?,
        Path::new("/mnt/data"),
        HostMountAccess::ReadWrite,
    ));
    assert_eq!(args[3], "kept/");
    Ok(())
}

#[test]
fn default_config_matches_the_running_platform() {
    let config = FuseMountConfig::default();
    assert_eq!(config.platform, FusePlatform::current());
    assert_eq!(config.other_users, FuseOtherUsers::default());
}

#[test]
fn mount_table_parsing_requires_a_fuse_entry_at_the_path() {
    let dir = Path::new("/mnt/data");
    assert!(parse_proc_mounts_for_fuse(
        "mountpoint-s3 /mnt/data fuse.mountpoint-s3 ro 0 0\n",
        dir
    ));
    assert!(!parse_proc_mounts_for_fuse(
        "mountpoint-s3 /other fuse.mountpoint-s3 ro 0 0\n",
        dir
    ));
    assert!(!parse_proc_mounts_for_fuse("tmpfs /mnt/data tmpfs rw 0 0\n", dir));
    assert!(!parse_proc_mounts_for_fuse("", dir));
}

#[test]
fn metrics_register_and_render() -> TestResult {
    let metrics = FuseMetrics::new()?;
    metrics.fuse_mount_total.with_label_values(&["b"]).inc();
    metrics
        .fuse_mount_duration_seconds
        .with_label_values(&["b"])
        .observe(0.5);
    let text = metrics.gather_text()?;
    assert!(text.contains("trogon_sandbox_volumes_fuse_mount_total"));
    assert!(text.contains("trogon_sandbox_volumes_fuse_mount_duration_seconds"));
    Ok(())
}

#[tokio::test]
async fn unsupported_platforms_refuse_to_mount() -> TestResult {
    let dir = tempdir()?;
    let manager = FuseMountManager::new(
        FuseMountConfig {
            platform: FusePlatform::Unsupported,
            ..FuseMountConfig::default()
        },
        FuseMetrics::new()?,
    );
    let result = manager
        .mount(&source("")?, &dir.path().join("m"), HostMountAccess::ReadOnly)
        .await;
    assert!(matches!(result, Err(VolumeError::FuseUnsupported)));
    Ok(())
}

#[tokio::test]
async fn mount_waits_for_the_mount_table_and_unmounts() -> TestResult {
    let dir = tempdir()?;
    let host = dir.path().join("mnt");
    std::fs::write(
        dir.path().join("mounts"),
        format!("mountpoint-s3 {} fuse.mountpoint-s3 rw 0 0\n", host.display()),
    )?;
    let metrics = FuseMetrics::new()?;
    let manager = FuseMountManager::new(config(dir.path(), "exit 0")?, metrics.clone());

    let handle = manager.mount(&source("p")?, &host, HostMountAccess::ReadWrite).await?;
    assert_eq!(handle.host_dir, host);
    assert_eq!(metrics.fuse_mount_total.with_label_values(&["tenant-bucket"]).get(), 1);

    handle.unmount().await?;
    assert!(!host.exists());
    Ok(())
}

#[tokio::test]
async fn mount_times_out_when_the_table_never_lists_it() -> TestResult {
    let dir = tempdir()?;
    let manager = FuseMountManager::new(config(dir.path(), "exit 0")?, FuseMetrics::new()?);
    let result = manager
        .mount(&source("")?, &dir.path().join("mnt"), HostMountAccess::ReadOnly)
        .await;
    assert!(matches!(result, Err(VolumeError::FuseMountTimeout { .. })));
    Ok(())
}

#[tokio::test]
async fn mount_reports_a_failing_command() -> TestResult {
    let dir = tempdir()?;
    let manager = FuseMountManager::new(config(dir.path(), "echo denied >&2; exit 3")?, FuseMetrics::new()?);
    let result = manager
        .mount(&source("")?, &dir.path().join("mnt"), HostMountAccess::ReadOnly)
        .await;
    match result {
        Err(VolumeError::FuseMountRejected { stderr, .. }) => assert!(stderr.contains("denied")),
        other => return Err(format!("unexpected result: {:?}", other.err()).into()),
    }
    Ok(())
}

#[tokio::test]
async fn mount_reports_a_missing_command() -> TestResult {
    let dir = tempdir()?;
    let mut config = config(dir.path(), "exit 0")?;
    config.mount_s3_binary = dir.path().join("absent");
    let manager = FuseMountManager::new(config, FuseMetrics::new()?);
    let result = manager
        .mount(&source("")?, &dir.path().join("mnt"), HostMountAccess::ReadOnly)
        .await;
    assert!(matches!(result, Err(VolumeError::FuseSpawnFailed(_))));
    Ok(())
}

#[tokio::test]
async fn unmount_falls_back_and_then_fails() -> TestResult {
    let dir = tempdir()?;
    let host = dir.path().join("mnt");
    std::fs::write(
        dir.path().join("mounts"),
        format!("m {} fuse.x rw 0 0\n", host.display()),
    )?;

    let mut failing = config(dir.path(), "exit 0")?;
    failing.unmount_binary = script(dir.path(), "fusermount-bad", "exit 1")?;
    let handle = FuseMountManager::new(failing, FuseMetrics::new()?)
        .mount(&source("")?, &host, HostMountAccess::ReadOnly)
        .await?;
    assert!(matches!(
        handle.unmount().await,
        Err(VolumeError::FuseUnmountFailed { .. })
    ));

    let mut fallback = config(dir.path(), "exit 0")?;
    fallback.unmount_binary = dir.path().join("absent");
    fallback.fallback_unmount_binary = script(dir.path(), "umount-ok", "exit 0")?;
    let handle = FuseMountManager::new(fallback, FuseMetrics::new()?)
        .mount(&source("")?, &host, HostMountAccess::ReadOnly)
        .await?;
    handle.unmount().await?;
    assert!(!host.exists());
    Ok(())
}
