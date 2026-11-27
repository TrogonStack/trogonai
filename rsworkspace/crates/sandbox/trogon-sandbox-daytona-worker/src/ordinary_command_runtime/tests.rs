use std::io::Write as _;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn elf(machine: u16, program_type: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; 64 + 56];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
    bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
    bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
    bytes[64..68].copy_from_slice(&program_type.to_le_bytes());
    bytes
}

fn artifact(bytes: &[u8]) -> Result<(tempfile::TempDir, std::path::PathBuf, String), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("supervisor");
    std::fs::write(&path, bytes)?;
    Ok((directory, path, Sha256Pin::of(bytes).as_str().to_owned()))
}

#[test]
fn a_pinned_static_executable_loads() -> TestResult {
    let (_directory, path, pin) = artifact(&elf(62, 1))?;
    assert!(OrdinaryCommandRuntime::load(&path, &pin).is_ok());
    Ok(())
}

#[test]
fn an_uppercase_pin_is_refused_before_the_file_is_touched() {
    let result = OrdinaryCommandRuntime::load(Path::new("/nonexistent"), &"AB".repeat(32));
    assert!(matches!(result, Err(OrdinaryCommandRuntimeError::PinNotLowercaseHex)));
}

#[test]
fn a_wrong_pin_is_refused() -> TestResult {
    let (_directory, path, _) = artifact(&elf(62, 1))?;
    let result = OrdinaryCommandRuntime::load(&path, &"00".repeat(32));
    assert!(matches!(result, Err(OrdinaryCommandRuntimeError::PinMismatch)));
    Ok(())
}

#[test]
fn a_directory_or_a_missing_path_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let pin = "00".repeat(32);
    assert!(matches!(
        OrdinaryCommandRuntime::load(directory.path(), &pin),
        Err(OrdinaryCommandRuntimeError::NotBoundedRegularFile)
    ));
    assert!(matches!(
        OrdinaryCommandRuntime::load(&directory.path().join("missing"), &pin),
        Err(OrdinaryCommandRuntimeError::Open(_))
    ));
    Ok(())
}

#[test]
fn an_oversized_artifact_is_refused() -> TestResult {
    let bytes = vec![0u8; MAX_ARTIFACT_BYTES + 1];
    let (_directory, path, pin) = artifact(&bytes)?;
    assert!(matches!(
        OrdinaryCommandRuntime::load(&path, &pin),
        Err(OrdinaryCommandRuntimeError::NotBoundedRegularFile)
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_is_refused() -> TestResult {
    let (directory, path, pin) = artifact(&elf(62, 1))?;
    let link = directory.path().join("link");
    std::os::unix::fs::symlink(&path, &link)?;
    assert!(matches!(
        OrdinaryCommandRuntime::load(&link, &pin),
        Err(OrdinaryCommandRuntimeError::NotBoundedRegularFile)
    ));
    Ok(())
}

#[test]
fn a_file_that_is_not_a_linux_executable_is_refused() -> TestResult {
    let (_directory, path, pin) = artifact(b"#!/bin/sh\nexit 0\n")?;
    assert!(matches!(
        OrdinaryCommandRuntime::load(&path, &pin),
        Err(OrdinaryCommandRuntimeError::NotLinuxElf64)
    ));
    Ok(())
}

#[test]
fn a_dynamically_linked_executable_is_refused() -> TestResult {
    let (_directory, path, pin) = artifact(&elf(183, 3))?;
    assert!(matches!(
        OrdinaryCommandRuntime::load(&path, &pin),
        Err(OrdinaryCommandRuntimeError::GuestInterpreter)
    ));
    Ok(())
}

#[test]
fn the_bundled_bridge_is_valid_javascript() -> TestResult {
    let Ok(node) = which_node() else {
        return Ok(());
    };
    let mut file = tempfile::Builder::new().suffix(".cjs").tempfile()?;
    file.write_all(BRIDGE.as_bytes())?;
    let checked = std::process::Command::new(node)
        .arg("--check")
        .arg(file.path())
        .output()?;
    assert!(checked.status.success(), "{}", String::from_utf8_lossy(&checked.stderr));
    Ok(())
}

#[test]
fn the_bundled_bridge_refuses_to_start_without_its_arguments() -> TestResult {
    let Ok(node) = which_node() else {
        return Ok(());
    };
    let mut file = tempfile::Builder::new().suffix(".cjs").tempfile()?;
    file.write_all(BRIDGE.as_bytes())?;
    let status = std::process::Command::new(node).arg(file.path()).status()?;
    assert_eq!(status.code(), Some(78));
    Ok(())
}

fn which_node() -> Result<&'static str, ()> {
    match std::process::Command::new("node").arg("--version").output() {
        Ok(output) if output.status.success() => Ok("node"),
        _ => Err(()),
    }
}
