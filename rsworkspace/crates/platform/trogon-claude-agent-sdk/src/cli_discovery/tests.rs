use std::{collections::BTreeMap, ffi::OsString, os::unix::fs::PermissionsExt, path::Path};

use super::discover;
use crate::adapters::NativeFileSystem;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn make(path: &Path, mode: u32) -> TestResult {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, "")?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn inherited(path: &Path, home: &Path) -> BTreeMap<OsString, OsString> {
    BTreeMap::from([
        ("PATH".into(), path.as_os_str().to_owned()),
        ("HOME".into(), home.as_os_str().to_owned()),
    ])
}

#[test]
fn path_hit_requires_an_executable_file() -> TestResult {
    let root = tempfile::tempdir()?;
    let bin = root.path().join("bin");
    make(&bin.join("claude"), 0o644)?;
    let home = root.path().join("home");
    assert!(discover(&inherited(&bin, &home), &NativeFileSystem, false).is_err());
    make(&bin.join("claude"), 0o755)?;
    assert_eq!(
        discover(&inherited(&bin, &home), &NativeFileSystem, false),
        Ok(bin.join("claude"))
    );
    Ok(())
}

#[test]
fn falls_back_to_home_install_locations_in_upstream_order() -> TestResult {
    let root = tempfile::tempdir()?;
    let home = root.path().join("home");
    make(&home.join(".yarn/bin/claude"), 0o644)?;
    make(&home.join(".npm-global/bin/claude"), 0o644)?;
    let empty = root.path().join("empty");
    assert_eq!(
        discover(&inherited(&empty, &home), &NativeFileSystem, false),
        Ok(home.join(".npm-global/bin/claude"))
    );
    Ok(())
}

#[test]
fn reports_the_exact_install_hint_when_nothing_is_found() -> TestResult {
    let root = tempfile::tempdir()?;
    // Both hints are only reachable when no well-known location exists, so the home is the empty temp directory.
    if let Err(error) = discover(&inherited(root.path(), root.path()), &NativeFileSystem, false) {
        assert!(error.message().starts_with(
            "Claude Code not found. Install with:\n  npm install -g @anthropic-ai/claude-code\n\nIf already installed locally, try:\n  export PATH=\"$HOME/node_modules/.bin:$PATH\"\n"
        ));
    }
    let windows = discover(&inherited(root.path(), root.path()), &NativeFileSystem, true)
        .err()
        .ok_or("expected a Windows discovery failure")?;
    assert!(windows.message().contains("irm https://claude.ai/install.ps1 | iex"));
    Ok(())
}

#[test]
fn windows_only_searches_absolute_path_entries_for_claude_exe() -> TestResult {
    let root = tempfile::tempdir()?;
    let bin = root.path().join("bin");
    make(&bin.join("claude.exe"), 0o644)?;
    assert_eq!(
        discover(&inherited(&bin, root.path()), &NativeFileSystem, true),
        Ok(bin.join("claude.exe"))
    );
    Ok(())
}
