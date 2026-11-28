use std::fs;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn every_regular_file_is_listed() -> TestResult {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("nested"))?;
    fs::write(root.path().join("a.txt"), "a")?;
    fs::write(root.path().join("nested").join("b.txt"), "b")?;
    let mut listed = files_under(root.path())?;
    listed.sort();
    assert_eq!(
        listed,
        vec![root.path().join("a.txt"), root.path().join("nested").join("b.txt")]
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_unreadable_directory_fails_the_walk_instead_of_being_skipped() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir()?;
    let locked = root.path().join("locked");
    fs::create_dir(&locked)?;
    fs::write(locked.join("secret.txt"), "x")?;
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))?;
    let readable_anyway = fs::read_dir(&locked).is_ok();
    let outcome = files_under(root.path());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755))?;
    if readable_anyway {
        return Ok(());
    }
    assert!(matches!(outcome, Err(ModalApiError::Io(_))));
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_symlink_fails_the_walk_and_names_its_path() -> TestResult {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("a.txt"), "a")?;
    let link = root.path().join("link.txt");
    std::os::unix::fs::symlink(root.path().join("a.txt"), &link)?;
    let outcome = files_under(root.path());
    assert!(
        matches!(&outcome, Err(ModalApiError::SymlinkInUpload(path)) if *path == link),
        "{outcome:?}"
    );
    Ok(())
}
