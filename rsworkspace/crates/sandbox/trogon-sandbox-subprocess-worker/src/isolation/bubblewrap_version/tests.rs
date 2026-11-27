use super::*;
use std::os::unix::fs::PermissionsExt;

fn version(text: &str) -> Result<BubblewrapVersion, Box<dyn std::error::Error>> {
    Ok(BubblewrapVersion::parse(text).ok_or("a version")?)
}

fn fake_bwrap(dir: &Path, body: &str) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let path = dir.join("bwrap");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

#[test]
fn parses_the_version_out_of_the_banner() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(version("bubblewrap 0.9.0")?, version("0.9.0")?);
    assert_eq!(version("bubblewrap 0.4")?, version("0.4.0")?);
    assert!(BubblewrapVersion::parse("no digits here").is_none());
    Ok(())
}

#[test]
fn compares_numerically_with_missing_components_as_zero() -> Result<(), Box<dyn std::error::Error>> {
    assert!(version("0.4.1")? < BubblewrapVersion::oldest_supported());
    assert!(version("0.10.0")? > version("0.9.9")?);
    assert_eq!(version("0.5")?, BubblewrapVersion::oldest_supported());
    Ok(())
}

#[test]
fn a_bubblewrap_older_than_the_minimum_is_outdated_but_still_launches() -> Result<(), Box<dyn std::error::Error>> {
    assert!(version("0.4.1")?.is_outdated());
    assert!(!version("0.5.0")?.is_outdated());
    let dir = tempfile::tempdir()?;
    let bwrap = fake_bwrap(dir.path(), "echo 'bubblewrap 0.4.1'")?;
    BubblewrapVersion::warn_if_outdated(&bwrap);
    Ok(())
}

#[test]
fn an_unknown_version_does_not_stop_the_launch() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let bwrap = fake_bwrap(dir.path(), "exit 1")?;
    BubblewrapVersion::warn_if_outdated(&bwrap);
    BubblewrapVersion::warn_if_outdated(&dir.path().join("missing"));
    Ok(())
}
