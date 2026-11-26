use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::ffi::OsStr;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt;

use tempfile::tempdir;
use trogon_sandbox_volumes::{ChangedManifest, ManifestKey, VolumeError, build_changed_manifest, compute_tree_digest};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn write_tree(root: &std::path::Path) -> TestResult {
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(root.join("README.md"), b"# demo")?;
    std::fs::write(root.join("src/main.rs"), b"fn main() {}")?;
    std::fs::write(root.join("src/lib.rs"), b"pub fn ok() {}")?;
    Ok(())
}

#[test]
fn manifest_lists_every_file_sorted_without_baseline() -> TestResult {
    let dir = tempdir()?;
    write_tree(dir.path())?;
    let manifest = build_changed_manifest(dir.path(), None)?;
    let keys: Vec<&str> = manifest.entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(keys, ["README.md", "src/lib.rs", "src/main.rs"]);
    assert_eq!(manifest.entries[0].size_bytes, 6);
    assert!(manifest.entries[0].digest.as_str().starts_with("sha256:"));
    Ok(())
}

#[test]
fn manifest_skips_files_matching_the_baseline() -> TestResult {
    let dir = tempdir()?;
    write_tree(dir.path())?;
    let full = build_changed_manifest(dir.path(), None)?;
    let baseline: BTreeMap<ManifestKey, _> = full.entries.iter().map(|e| (e.key.clone(), e.digest.clone())).collect();

    assert!(build_changed_manifest(dir.path(), Some(&baseline))?.entries.is_empty());

    std::fs::write(dir.path().join("README.md"), b"# changed")?;
    std::fs::write(dir.path().join("new.txt"), b"new")?;
    let changed = build_changed_manifest(dir.path(), Some(&baseline))?;
    let keys: Vec<&str> = changed.entries.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(keys, ["README.md", "new.txt"]);
    Ok(())
}

#[test]
fn manifest_digest_depends_on_keys_and_digests() -> TestResult {
    let dir = tempdir()?;
    write_tree(dir.path())?;
    let manifest = build_changed_manifest(dir.path(), None)?;
    let first = manifest.digest()?;
    assert_eq!(first, manifest.clone().digest()?);

    std::fs::write(dir.path().join("README.md"), b"# changed")?;
    assert_ne!(first, build_changed_manifest(dir.path(), None)?.digest()?);
    assert!(ChangedManifest::default().digest()?.as_str().starts_with("sha256:"));
    Ok(())
}

#[test]
fn tree_digest_is_stable_and_content_sensitive() -> TestResult {
    let a = tempdir()?;
    let b = tempdir()?;
    write_tree(a.path())?;
    write_tree(b.path())?;
    assert_eq!(compute_tree_digest(a.path())?, compute_tree_digest(b.path())?);

    std::fs::write(b.path().join("src/lib.rs"), b"changed")?;
    assert_ne!(compute_tree_digest(a.path())?, compute_tree_digest(b.path())?);
    Ok(())
}

#[test]
fn missing_root_is_an_io_error() -> TestResult {
    let dir = tempdir()?;
    let missing = dir.path().join("missing");
    assert!(matches!(
        build_changed_manifest(&missing, None),
        Err(VolumeError::Io(_))
    ));
    assert!(matches!(compute_tree_digest(&missing), Err(VolumeError::Io(_))));
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_followed() -> TestResult {
    let dir = tempdir()?;
    write_tree(dir.path())?;
    std::os::unix::fs::symlink(dir.path().join("README.md"), dir.path().join("link"))?;
    let manifest = build_changed_manifest(dir.path(), None)?;
    assert_eq!(manifest.entries.len(), 3);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_file_names_are_key_errors() -> TestResult {
    let dir = tempdir()?;
    std::fs::write(dir.path().join(OsStr::from_bytes(&[0x66, 0xff])), b"x")?;
    assert!(matches!(
        build_changed_manifest(dir.path(), None),
        Err(VolumeError::Key(_))
    ));
    Ok(())
}
