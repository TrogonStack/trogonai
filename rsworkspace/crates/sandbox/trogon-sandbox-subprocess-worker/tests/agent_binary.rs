#![cfg(unix)]

use std::error::Error;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use trogon_sandbox_contract::{OciImageRef, SandboxImage};
use trogon_sandbox_subprocess_worker::lease_launch::resolve_agent_binary;

const EXECUTABLE: &[u8] = b"#!/bin/sh\nexit 0\n";

type TestResult = Result<(), Box<dyn Error>>;

fn executable(path: &Path) -> TestResult {
    fs::write(path, EXECUTABLE)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

/// A registry image whose repository is the given executable path, or nothing when empty.
fn image_at(path: &str) -> SandboxImage {
    SandboxImage::Registry {
        reference: OciImageRef {
            registry: String::new(),
            repository: path.to_string(),
            tag: None,
            digest: None,
        },
        pull_secret: None,
    }
}

fn path_str(path: &Path) -> Result<&str, Box<dyn Error>> {
    Ok(path.to_str().ok_or("path is not UTF-8")?)
}

#[test]
fn configured_default_executable_alias_resolves_to_its_physical_path() -> TestResult {
    let root = tempfile::tempdir()?;
    let physical = root.path().join("physical-agent");
    executable(&physical)?;
    let alias = root.path().join("configured-agent");
    symlink(&physical, &alias)?;

    let resolved = resolve_agent_binary(&alias, &image_at(""))?;

    assert_eq!(
        resolved,
        physical.canonicalize()?,
        "launch must use the validated executable's physical path"
    );
    assert_eq!(fs::read(&physical)?, EXECUTABLE);
    assert!(fs::symlink_metadata(&alias)?.file_type().is_symlink());
    Ok(())
}

#[test]
fn explicit_executable_alias_resolves_to_its_physical_path() -> TestResult {
    let root = tempfile::tempdir()?;
    let physical = root.path().join("physical-agent");
    executable(&physical)?;
    let alias = root.path().join("requested-agent");
    symlink(&physical, &alias)?;
    let image = image_at(path_str(&alias)?);

    let resolved = resolve_agent_binary(&root.path().join("unselected-default"), &image)?;

    assert_eq!(
        resolved,
        physical.canonicalize()?,
        "the requested executable must not retain an alias through a denied directory"
    );
    assert_eq!(fs::read(&physical)?, EXECUTABLE);
    assert!(fs::symlink_metadata(&alias)?.file_type().is_symlink());
    Ok(())
}

#[test]
fn executable_aliases_do_not_admit_missing_directory_or_nonexecutable_targets() -> TestResult {
    let root = tempfile::tempdir()?;
    let directory = root.path().join("directory");
    fs::create_dir(&directory)?;
    let nonexecutable = root.path().join("nonexecutable");
    fs::write(&nonexecutable, EXECUTABLE)?;
    fs::set_permissions(&nonexecutable, fs::Permissions::from_mode(0o644))?;
    let missing = root.path().join("missing");

    for (name, target) in [
        ("directory-alias", &directory),
        ("nonexecutable-alias", &nonexecutable),
        ("missing-alias", &missing),
    ] {
        let alias = root.path().join(name);
        symlink(target, &alias)?;
        for image in [image_at(""), image_at(path_str(&alias)?)] {
            assert!(resolve_agent_binary(&alias, &image).is_err(), "{name} must be refused");
        }
    }
    assert_eq!(fs::read(&nonexecutable)?, EXECUTABLE);
    assert!(!missing.exists());
    Ok(())
}
