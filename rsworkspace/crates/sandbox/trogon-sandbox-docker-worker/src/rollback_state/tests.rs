#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use trogon_sandbox_contract::LeaseId;

use super::*;
use crate::support_tests::{TestResult, lease_request};

fn request(key: &str) -> Result<LeaseRequest, Box<dyn std::error::Error>> {
    lease_request(key)
}

#[tokio::test]
async fn a_remembered_request_survives_a_new_store_on_the_same_directory() -> TestResult {
    let dir = tempfile::tempdir()?;
    let lease = LeaseId::new("lease/1");
    let first = RollbackStateStore::open(dir.path(), Duration::from_secs(60))?;
    first.remember(&lease, &request("img1")?).await;

    let second = RollbackStateStore::open(dir.path(), Duration::from_secs(60))?;
    assert_eq!(second.load(&lease).await, Some(request("img1")?));
    Ok(())
}

#[tokio::test]
async fn forgetting_removes_the_entry() -> TestResult {
    let dir = tempfile::tempdir()?;
    let lease = LeaseId::new("l");
    let store = RollbackStateStore::open(dir.path(), Duration::from_secs(60))?;
    store.remember(&lease, &request("img1")?).await;
    store.forget(&lease).await;
    store.forget(&lease).await;
    assert_eq!(store.load(&lease).await, None);
    Ok(())
}

#[tokio::test]
async fn the_latest_request_wins() -> TestResult {
    let dir = tempfile::tempdir()?;
    let lease = LeaseId::new("l");
    let store = RollbackStateStore::open(dir.path(), Duration::from_secs(60))?;
    store.remember(&lease, &request("img1")?).await;
    store.remember(&lease, &request("img2")?).await;
    assert_eq!(store.load(&lease).await, Some(request("img2")?));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn entries_older_than_the_retention_are_pruned() -> TestResult {
    let dir = tempfile::tempdir()?;
    let lease = LeaseId::new("l");
    let store = RollbackStateStore::open(dir.path(), Duration::from_secs(60))?;
    store.remember(&lease, &request("img1")?).await;
    let path = store.path(&lease).ok_or("path")?;
    let old = std::time::SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options().write(true).open(&path)?.set_modified(old)?;
    let reopened = RollbackStateStore::open(dir.path(), Duration::from_secs(60))?;
    assert_eq!(reopened.load(&lease).await, None);
    Ok(())
}

#[tokio::test]
async fn recent_entries_survive_a_prune() -> TestResult {
    let dir = tempfile::tempdir()?;
    let lease = LeaseId::new("l");
    let store = RollbackStateStore::open(dir.path(), Duration::from_secs(3600))?;
    store.remember(&lease, &request("img1")?).await;
    let reopened = RollbackStateStore::open(dir.path(), Duration::from_secs(3600))?;
    assert_eq!(reopened.load(&lease).await, Some(request("img1")?));
    Ok(())
}

#[tokio::test]
async fn an_unreadable_entry_loads_as_nothing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let lease = LeaseId::new("l");
    let store = RollbackStateStore::open(dir.path(), Duration::from_secs(60))?;
    std::fs::write(store.path(&lease).ok_or("path")?, b"not json")?;
    assert_eq!(store.load(&lease).await, None);
    Ok(())
}

#[tokio::test]
async fn a_failed_write_is_logged_and_the_entry_stays_missing() -> TestResult {
    let dir = tempfile::tempdir()?;
    let lease = LeaseId::new("l");
    let store = RollbackStateStore::open(dir.path(), Duration::from_secs(60))?;
    std::fs::remove_dir_all(dir.path().join("rollback"))?;
    store.remember(&lease, &request("img1")?).await;
    assert_eq!(store.load(&lease).await, None);
    Ok(())
}

#[tokio::test]
async fn a_disabled_store_remembers_nothing() -> TestResult {
    let store = RollbackStateStore::disabled();
    let lease = LeaseId::new("l");
    store.remember(&lease, &request("img1")?).await;
    store.forget(&lease).await;
    assert!(!store.is_enabled());
    assert_eq!(store.load(&lease).await, None);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn files_are_private() -> TestResult {
    let dir = tempfile::tempdir()?;
    let lease = LeaseId::new("l");
    let store = RollbackStateStore::open(dir.path(), Duration::from_secs(60))?;
    store.remember(&lease, &request("img1")?).await;
    let path = store.path(&lease).ok_or("path")?;
    let mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let parent = path.parent().ok_or("parent")?;
    assert_eq!(std::fs::metadata(parent)?.permissions().mode() & 0o777, 0o700);
    Ok(())
}
