use std::{collections::BTreeMap, ffi::OsString, os::unix::fs::PermissionsExt, path::Path, path::PathBuf};

use super::{CliDiscoveryError, discover};
use crate::{
    Error,
    adapters::NativeFileSystem,
    test_support::{FixedEnvironment, WithPasswd},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn make(path: &Path, mode: u32) -> TestResult {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, "")?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn inherited(path: &Path) -> BTreeMap<OsString, OsString> {
    BTreeMap::from([("PATH".into(), path.as_os_str().to_owned())])
}

/// A process environment whose `HOME` and `USERPROFILE` are `home` when given, and whose passwd database has an
/// entry for both users only when `passwd` is given.
fn environment(home: Option<&Path>, passwd: Option<&Path>) -> WithPasswd<FixedEnvironment> {
    WithPasswd {
        environment: FixedEnvironment {
            directory: PathBuf::from("/work"),
            variables: home
                .into_iter()
                .flat_map(|home| {
                    [
                        (OsString::from("HOME"), home.as_os_str().to_owned()),
                        (OsString::from("USERPROFILE"), home.as_os_str().to_owned()),
                    ]
                })
                .collect(),
        },
        effective: passwd.map(|path| path.as_os_str().to_owned()),
        real: passwd.map(|path| path.as_os_str().to_owned()),
    }
}

#[test]
fn path_hit_requires_an_executable_file() -> TestResult {
    let root = tempfile::tempdir()?;
    let bin = root.path().join("bin");
    make(&bin.join("claude"), 0o644)?;
    let environment = environment(Some(&root.path().join("home")), None);
    assert!(discover(&environment, &inherited(&bin), &NativeFileSystem, false).is_err());
    make(&bin.join("claude"), 0o755)?;
    assert_eq!(
        discover(&environment, &inherited(&bin), &NativeFileSystem, false),
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
        discover(
            &environment(Some(&home), None),
            &inherited(&empty),
            &NativeFileSystem,
            false
        ),
        Ok(home.join(".npm-global/bin/claude"))
    );
    Ok(())
}

#[test]
fn reports_the_exact_install_hint_when_nothing_is_found() -> TestResult {
    let root = tempfile::tempdir()?;
    // Both hints are only reachable when no well-known location exists, so the home is the empty temp directory.
    let environment = environment(Some(root.path()), None);
    if let Err(CliDiscoveryError::NotFound(error)) =
        discover(&environment, &inherited(root.path()), &NativeFileSystem, false)
    {
        assert!(error.message().starts_with(
            "Claude Code not found. Install with:\n  npm install -g @anthropic-ai/claude-code\n\nIf already installed locally, try:\n  export PATH=\"$HOME/node_modules/.bin:$PATH\"\n"
        ));
    }
    let windows = discover(&environment, &inherited(root.path()), &NativeFileSystem, true);
    let Err(CliDiscoveryError::NotFound(windows)) = windows else {
        return Err("expected a Windows discovery failure".into());
    };
    assert!(windows.message().contains("irm https://claude.ai/install.ps1 | iex"));
    Ok(())
}

#[test]
fn windows_only_searches_absolute_path_entries_for_claude_exe() -> TestResult {
    let root = tempfile::tempdir()?;
    let bin = root.path().join("bin");
    make(&bin.join("claude.exe"), 0o644)?;
    assert_eq!(
        discover(
            &environment(Some(root.path()), None),
            &inherited(&bin),
            &NativeFileSystem,
            true
        ),
        Ok(bin.join("claude.exe"))
    );
    Ok(())
}

#[test]
fn a_path_hit_needs_no_home_directory() -> TestResult {
    let root = tempfile::tempdir()?;
    let bin = root.path().join("bin");
    make(&bin.join("claude"), 0o755)?;
    make(&bin.join("claude.exe"), 0o644)?;
    let nowhere = environment(None, None);
    assert_eq!(
        discover(&nowhere, &inherited(&bin), &NativeFileSystem, false),
        Ok(bin.join("claude"))
    );
    assert_eq!(
        discover(&nowhere, &inherited(&bin), &NativeFileSystem, true),
        Ok(bin.join("claude.exe"))
    );
    Ok(())
}

#[test]
fn a_missing_home_raises_after_the_path_lookup_like_path_home() -> TestResult {
    let root = tempfile::tempdir()?;
    let nowhere = environment(None, None);
    for windows in [false, true] {
        let error = discover(&nowhere, &inherited(root.path()), &NativeFileSystem, windows)
            .err()
            .ok_or("expected the home lookup to fail")?;
        let CliDiscoveryError::Home(home) = &error else {
            return Err("expected a home directory error".into());
        };
        assert_eq!(home.message(), "Could not determine home directory.");
        assert!(matches!(Error::from(error), Error::HomeDirectory(_)));
    }
    Ok(())
}

#[test]
fn an_unset_home_searches_under_the_passwd_entry_of_the_real_user() -> TestResult {
    let root = tempfile::tempdir()?;
    let passwd_home = root.path().join("passwd-home");
    make(&passwd_home.join(".local/bin/claude"), 0o644)?;
    let empty = root.path().join("empty");
    assert_eq!(
        discover(
            &environment(None, Some(&passwd_home)),
            &inherited(&empty),
            &NativeFileSystem,
            false
        ),
        Ok(passwd_home.join(".local/bin/claude"))
    );
    Ok(())
}

#[test]
fn a_not_found_failure_converts_to_the_public_error() {
    let error = CliDiscoveryError::NotFound(super::CliNotFoundError { message: "missing" });
    assert!(matches!(Error::from(error), Error::CliNotFound(found) if found.message() == "missing"));
}
