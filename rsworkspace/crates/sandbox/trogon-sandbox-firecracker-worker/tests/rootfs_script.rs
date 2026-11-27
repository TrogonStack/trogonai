#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use sha2::{Digest as _, Sha256};
use trogon_std::env::{ReadEnv, SystemEnv};

fn squashfs() -> &'static [u8] {
    b"not really a squashfs"
}

fn run(dir: &Path, extra: &[&str], env_sha: Option<&str>) -> Result<Output, Box<dyn std::error::Error>> {
    let agent = dir.join("agent");
    std::fs::write(&agent, b"#!/bin/sh\n")?;
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755))?;
    std::fs::write(dir.join("base.squashfs"), squashfs())?;
    // Stubs stand in for the image tools so nothing is unpacked or run under sudo.
    let bin = dir.join("stubs");
    std::fs::create_dir_all(&bin)?;
    for tool in ["sudo", "unsquashfs", "mkfs.ext4", "curl", "truncate"] {
        let path = bin.join(tool);
        std::fs::write(&path, b"#!/bin/sh\nexit 42\n")?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    let path_env = format!("{}:{}", bin.display(), SystemEnv.var("PATH").unwrap_or_default());
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/build-guest-rootfs.sh");
    let mut command = Command::new("bash");
    command
        .arg(script)
        .args(["--agent-bin"])
        .arg(&agent)
        .args(["--out"])
        .arg(dir.join("rootfs.ext4"))
        .args(["--squashfs"])
        .arg(dir.join("base.squashfs"))
        .args(extra)
        .env("PATH", path_env)
        .env_remove("SANDBOX_SQUASHFS_SHA256");
    if let Some(sha) = env_sha {
        command.env("SANDBOX_SQUASHFS_SHA256", sha);
    }
    Ok(command.output()?)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn squashfs_without_a_digest_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let output = run(dir.path(), &[], None)?;
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--squashfs needs --squashfs-sha256"));
    Ok(())
}

#[test]
fn squashfs_with_a_wrong_digest_is_refused_and_kept() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let wrong = "0".repeat(64);
    let output = run(dir.path(), &["--squashfs-sha256", &wrong], None)?;
    assert!(!output.status.success());
    assert!(stderr(&output).contains("checksum mismatch"));
    assert!(dir.path().join("base.squashfs").exists());
    assert!(!dir.path().join("rootfs.ext4").exists());
    Ok(())
}

#[test]
fn env_digest_mismatch_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let wrong = "f".repeat(64);
    let output = run(dir.path(), &[], Some(&wrong))?;
    assert!(!output.status.success());
    assert!(stderr(&output).contains("checksum mismatch"));
    Ok(())
}

#[test]
fn matching_digest_passes_verification() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let right = hex::encode(Sha256::digest(squashfs())).to_uppercase();
    let output = run(dir.path(), &["--squashfs-sha256", &right], None)?;
    // The stubbed unpack step is the first thing to run after verification.
    // The script targets GNU coreutils; BSD readlink stops it earlier on other hosts.
    if cfg!(target_os = "linux") {
        assert_eq!(output.status.code(), Some(42), "{}", stderr(&output));
    }
    assert!(!stderr(&output).contains("checksum mismatch"));
    assert!(!stderr(&output).contains("--squashfs needs"));
    Ok(())
}
