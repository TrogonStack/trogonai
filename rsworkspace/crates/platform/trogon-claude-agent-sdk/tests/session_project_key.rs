#[cfg(unix)]
use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
use std::{
    io,
    path::{Path, PathBuf},
};
use trogon_claude_agent_sdk::{SessionProjectKey, adapters::CanonicalizePath};

struct CanonicalPath;
impl CanonicalizePath for CanonicalPath {
    type Error = io::Error;
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, Self::Error> {
        Ok(path.to_path_buf())
    }
}

#[test]
fn project_key_matches_upstream_utf16_replacement() -> Result<(), Box<dyn std::error::Error>> {
    // TypeScript composes the path to NFC only on macOS; elsewhere the combining mark is replaced on its own.
    let decomposed_cafe = if cfg!(target_os = "macos") {
        "-projects-caf-"
    } else {
        "-projects-cafe-"
    };
    for (path, expected) in [
        ("/projects/hello🌻", "-projects-hello--"),
        ("/projects/cafe\u{301}", decomposed_cafe),
        ("/projects/hello.world", "-projects-hello-world"),
    ] {
        let key = SessionProjectKey::for_directory_with_filesystem(Path::new(path), &CanonicalPath)?;
        assert_eq!(key.as_str(), expected);
    }
    let path = format!("{}🌻", "a".repeat(198));
    let key = SessionProjectKey::for_directory_with_filesystem(Path::new(&path), &CanonicalPath)?;
    assert_eq!(key.as_str(), format!("{}--", "a".repeat(198)));
    Ok(())
}

#[test]
fn verbatim_windows_prefixes_do_not_change_the_key() -> Result<(), Box<dyn std::error::Error>> {
    for (path, expected) in [
        (r"\\?\C:\Users\me\proj", "C--Users-me-proj"),
        (r"C:\Users\me\proj", "C--Users-me-proj"),
        (r"\\?\UNC\srv\share\proj", "--srv-share-proj"),
        (r"\\srv\share\proj", "--srv-share-proj"),
    ] {
        let key = SessionProjectKey::for_directory_with_filesystem(Path::new(path), &CanonicalPath)?;
        assert_eq!(key.as_str(), expected, "{path}");
    }
    Ok(())
}

#[test]
fn long_project_paths_are_truncated_and_suffixed_with_a_stable_hash() -> Result<(), Box<dyn std::error::Error>> {
    let path = "a".repeat(250);
    let key = SessionProjectKey::for_directory_with_filesystem(Path::new(&path), &CanonicalPath)?;
    assert_eq!(key.as_str(), format!("{}-wm9qhs", "a".repeat(200)));
    assert_eq!(key.to_string(), key.as_str());
    Ok(())
}

#[test]
fn explicit_project_keys_reject_unsafe_components() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(SessionProjectKey::new("project-1")?.as_str(), "project-1");
    for value in [
        "",
        ".",
        "..",
        "a/b",
        "a\\b",
        "a:b",
        "trailing.",
        "CON",
        "lpt1.txt",
        "tab\there",
    ] {
        assert!(SessionProjectKey::new(value).is_err(), "{value:?} must be rejected");
    }
    Ok(())
}

#[test]
fn native_directory_resolution_reports_missing_directories() {
    let error = SessionProjectKey::for_directory(Path::new("/nonexistent/claude-sdk-project-key"))
        .err()
        .map(|error| error.to_string());
    assert_eq!(error.as_deref(), Some("could not resolve the project directory"));
}

#[test]
fn native_directory_resolution_sanitizes_the_canonical_path() -> Result<(), Box<dyn std::error::Error>> {
    let key = SessionProjectKey::for_directory(Path::new("."))?;
    assert!(!key.as_str().is_empty());
    assert!(key.as_str().chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-'));
    Ok(())
}

#[cfg(unix)]
#[test]
fn non_unicode_directories_are_rejected() {
    let path = Path::new(OsStr::from_bytes(b"/projects/\xff"));
    let error = SessionProjectKey::for_directory_with_filesystem(path, &CanonicalPath)
        .err()
        .map(|error| error.to_string());
    assert_eq!(error.as_deref(), Some("project directory is not Unicode"));
}
