#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use trogon_sandbox_windows_fence::{DirLook, HostDisk, PathOnDisk, ProfileDisk};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn the_host_disk_lists_a_directory_and_reports_what_is_absent() -> TestResult {
    let temp = tempfile::tempdir()?;
    let dir = temp.path();
    std::fs::create_dir_all(dir.join("inner"))?;
    let text = dir.to_string_lossy().into_owned();
    assert_eq!(HostDisk.look(&text), DirLook::Names(vec!["inner".to_string()]));
    assert_eq!(HostDisk.look(&format!("{text}/nope")), DirLook::Absent);
    assert_eq!(HostDisk.look(&format!("{text}/inner/")), DirLook::Names(Vec::new()));
    assert_eq!(HostDisk.resolve(&format!("{text}/nope")), PathOnDisk::Absent);
    assert!(matches!(HostDisk.resolve(&text), PathOnDisk::Resolved(_)));
    Ok(())
}

#[test]
fn a_file_is_not_a_directory() -> TestResult {
    let temp = tempfile::tempdir()?;
    let file = temp.path().join("f");
    std::fs::write(&file, "x")?;
    let text = file.to_string_lossy().into_owned();
    assert_eq!(HostDisk.look(&text), DirLook::NotDirectory);
    assert_eq!(HostDisk.look(&format!("{text}/below")), DirLook::Absent);
    assert_eq!(HostDisk.resolve(&format!("{text}/below")), PathOnDisk::Absent);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_is_reported_as_a_link() -> TestResult {
    let temp = tempfile::tempdir()?;
    let target = temp.path().join("target");
    std::fs::create_dir(&target)?;
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&target, &link)?;
    assert_eq!(HostDisk.look(&link.to_string_lossy()), DirLook::Link);
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_unreadable_directory_is_reported_as_failed() -> TestResult {
    let temp = tempfile::tempdir()?;
    let dir = temp.path().join("locked");
    std::fs::create_dir(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000))?;
    let text = dir.to_string_lossy().into_owned();
    let look = HostDisk.look(&text);
    let below = HostDisk.look(&format!("{text}/x"));
    let resolved = HostDisk.resolve(&format!("{text}/x"));
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))?;
    // A superuser can read anything, so only assert when the lock held.
    if matches!(look, DirLook::Failed(_)) {
        assert!(matches!(below, DirLook::Failed(_)));
        assert!(matches!(resolved, PathOnDisk::Failed(_)));
    }
    Ok(())
}
