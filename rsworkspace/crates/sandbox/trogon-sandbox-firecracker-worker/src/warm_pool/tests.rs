use super::*;
use std::time::Duration;

fn key(memory: u32) -> Result<WarmKey, Box<dyn std::error::Error>> {
    let config = crate::config::FirecrackerConfig::new(
        crate::config::FirecrackerBinary::new("firecracker"),
        crate::config::KernelImage::new("kernel"),
        crate::config::BaseRootfs::new("base"),
        crate::config::StateDir::new("state"),
    );
    Ok(WarmKey::new(
        "base-image",
        &config,
        crate::config::MemoryMib::new(memory)?,
        None,
    ))
}

#[test]
fn disabled_pool_never_returns_a_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    let mut pool = WarmPool::new(WarmPoolConfig::default());
    assert!(!pool.enabled());
    assert!(pool.get(&key(512)?).is_none());
    Ok(())
}

#[tokio::test]
async fn eviction_removes_files_after_an_in_flight_restore_releases_them() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut pool = WarmPool::new(WarmPoolConfig {
        max_snapshots: 1,
        ..Default::default()
    });
    let first = LocalSnapshot::fixture(dir.path(), 1)?;
    let first_path = first.path().to_path_buf();
    pool.insert(key(1)?, first).await?;
    let held = pool.get(&key(1)?).ok_or("snapshot missing")?;
    pool.insert(key(2)?, LocalSnapshot::fixture(dir.path(), 2)?).await?;
    assert!(pool.get(&key(1)?).is_none());
    assert!(first_path.exists());
    drop(held);
    assert!(!first_path.exists());
    assert!(pool.get(&key(2)?).is_some());
    pool.clear();
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
    Ok(())
}

#[tokio::test]
async fn byte_budget_and_idle_expiry_remove_snapshot_data() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut pool = WarmPool::new(WarmPoolConfig {
        max_snapshots: 4,
        max_bytes: 1024 * 1024,
        ..Default::default()
    });
    pool.insert(key(1)?, LocalSnapshot::fixture(dir.path(), 1)?).await?;
    assert!(pool.get(&key(1)?).is_none());
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
    let mut pool = WarmPool::new(WarmPoolConfig {
        max_snapshots: 4,
        max_idle: Duration::ZERO,
        ..Default::default()
    });
    pool.insert(key(1)?, LocalSnapshot::fixture(dir.path(), 1)?).await?;
    assert!(pool.get(&key(1)?).is_none());
    assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
    Ok(())
}
