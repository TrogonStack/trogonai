use super::*;
use crate::support_tests::TestResult;

#[test]
fn creates_then_reuses_owner() -> TestResult {
    let dir = tempfile::tempdir()?;
    let first = load_or_create(dir.path())?.owner;
    assert!(valid(&first));
    let second = load_or_create(dir.path())?.owner;
    assert_eq!(first, second);
    Ok(())
}

#[test]
fn creates_missing_state_dir() -> TestResult {
    let dir = tempfile::tempdir()?;
    let owner = load_or_create(&dir.path().join("a/b"))?.owner;
    assert!(valid(&owner));
    Ok(())
}

#[test]
fn a_state_dir_that_cannot_be_created_is_reported() -> TestResult {
    let dir = tempfile::tempdir()?;
    let blocker = dir.path().join("file");
    std::fs::write(&blocker, "")?;
    let result = load_or_create(&blocker.join("state"));
    assert!(matches!(result, Err(OwnerStoreError::CreateDir { .. })));
    Ok(())
}

#[test]
fn an_unreadable_owner_record_is_reported() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join(OWNER_FILE))?;
    let result = load_or_create(dir.path());
    assert!(matches!(result, Err(OwnerStoreError::Io(_))));
    Ok(())
}

#[test]
fn different_state_dirs_get_different_owners() -> TestResult {
    let a = tempfile::tempdir()?;
    let b = tempfile::tempdir()?;
    let a = load_or_create(a.path())?;
    let b = load_or_create(b.path())?;
    assert_ne!(a.owner, b.owner);
    Ok(())
}

#[test]
fn empty_file_is_replaced() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join(OWNER_FILE), "")?;
    let owner = load_or_create(dir.path())?.owner;
    assert!(valid(&owner));
    assert_eq!(std::fs::read_to_string(dir.path().join(OWNER_FILE))?, owner);
    Ok(())
}

#[test]
fn corrupt_file_is_replaced() -> TestResult {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join(OWNER_FILE), "not an id\n\u{0}")?;
    let owner = load_or_create(dir.path())?.owner;
    assert!(valid(&owner));
    Ok(())
}

#[test]
fn trailing_newline_is_tolerated() -> TestResult {
    let dir = tempfile::tempdir()?;
    let id = crate::fresh_id::fresh_id();
    std::fs::write(dir.path().join(OWNER_FILE), format!("{id}\n"))?;
    assert_eq!(load_or_create(dir.path())?.owner, id);
    Ok(())
}

#[test]
fn second_worker_on_same_dir_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let _held = load_or_create(dir.path())?;
    assert!(matches!(load_or_create(dir.path()), Err(OwnerStoreError::Locked(_))));
    Ok(())
}
