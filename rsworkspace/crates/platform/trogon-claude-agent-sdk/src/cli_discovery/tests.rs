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
    assert!(discover(&inherited(&bin, &home), &NativeFileSystem, false, None).is_err());
    make(&bin.join("claude"), 0o755)?;
    assert_eq!(
        discover(&inherited(&bin, &home), &NativeFileSystem, false, None),
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
        discover(&inherited(&empty, &home), &NativeFileSystem, false, None),
        Ok(home.join(".npm-global/bin/claude"))
    );
    Ok(())
}

#[test]
fn reports_the_exact_install_hint_when_nothing_is_found() -> TestResult {
    let root = tempfile::tempdir()?;
    // Both hints are only reachable when no well-known location exists, so the home is the empty temp directory.
    if let Err(error) = discover(&inherited(root.path(), root.path()), &NativeFileSystem, false, None) {
        assert!(error.message().starts_with(
            "Claude Code not found. Install with:\n  npm install -g @anthropic-ai/claude-code\n\nIf already installed locally, try:\n  export PATH=\"$HOME/node_modules/.bin:$PATH\"\n"
        ));
    }
    let windows = discover(&inherited(root.path(), root.path()), &NativeFileSystem, true, None)
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
        discover(&inherited(&bin, root.path()), &NativeFileSystem, true, None),
        Ok(bin.join("claude.exe"))
    );
    Ok(())
}

const BUNDLED_GOLDENS: &str = include_str!("../../tests/fixtures/cli_discovery/python_bundled_lookup.json");

#[test]
fn the_bundled_directory_comes_first_like_python_and_only_when_named() -> TestResult {
    let goldens: serde_json::Value = serde_json::from_str(BUNDLED_GOLDENS)?;
    for case in goldens["cases"].as_array().ok_or("goldens have no cases")? {
        let name = case["name"].as_str().ok_or("case without a name")?;
        let windows = case["windows"] == true;
        let executable = if windows { "claude.exe" } else { "claude" };
        let root = tempfile::tempdir()?;
        let package = root.path().join("package");
        let bundled = package.join("_bundled").join(executable);
        std::fs::create_dir_all(package.join("_bundled"))?;
        match case["bundled"].as_str() {
            Some("file") => make(&bundled, 0o644)?,
            Some("executable") => make(&bundled, 0o755)?,
            Some("symlink") => {
                make(&root.path().join("target"), 0o644)?;
                std::os::unix::fs::symlink(root.path().join("target"), &bundled)?;
            }
            Some("dangling") => std::os::unix::fs::symlink(root.path().join("missing"), &bundled)?,
            Some("directory") => std::fs::create_dir(&bundled)?,
            Some("extensionless") => make(&package.join("_bundled/claude"), 0o644)?,
            None => {}
            Some(other) => return Err(format!("{name}: unknown bundled state {other}").into()),
        }
        let bin = root.path().join("bin");
        std::fs::create_dir_all(&bin)?;
        if case["path"] == true {
            make(&bin.join(executable), 0o755)?;
        }
        let home = root.path().join("home");
        std::fs::create_dir_all(&home)?;
        if case["home"] == true {
            make(&home.join(".local/bin").join(executable), 0o644)?;
        }
        let expected = match case["python"].as_str() {
            Some("bundled") => bundled.clone(),
            Some("path") => bin.join(executable),
            Some("home") => home.join(".local/bin").join(executable),
            other => return Err(format!("{name}: unexpected outcome {other:?}").into()),
        };
        let inherited = inherited(&bin, &home);
        assert_eq!(
            discover(&inherited, &NativeFileSystem, windows, Some(&package)),
            Ok(expected),
            "{name}"
        );
        // TypeScript has no bundled directory, so without a root the bundled executable is never taken.
        assert_ne!(
            discover(&inherited, &NativeFileSystem, windows, None).ok(),
            Some(bundled),
            "{name}"
        );
    }
    Ok(())
}
