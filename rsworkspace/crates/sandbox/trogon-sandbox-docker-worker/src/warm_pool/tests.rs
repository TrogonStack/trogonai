use super::*;
use crate::support_tests::TestResult;

fn digest(label: &str) -> Result<Digest, trogon_sandbox_contract::DigestError> {
    Digest::new(format!("sha256:{label:0>64}"))
}

fn container(id: &str, digest: &Digest) -> WarmContainer {
    WarmContainer::new(id.to_string(), digest.clone(), "img:tag".to_string())
}

#[test]
fn take_returns_none_for_unknown_digest() -> TestResult {
    let mut pool = WarmPool::new(WarmPoolConfig::default());
    assert!(pool.take(&digest("a")?).is_none());
    Ok(())
}

#[test]
fn population_omits_a_drained_digest() -> TestResult {
    let d = digest("d")?;
    let mut pool = WarmPool::new(WarmPoolConfig::default());
    pool.insert(container("c1", &d));
    assert!(pool.take(&d).is_some());
    assert!(pool.population().is_empty());
    Ok(())
}

#[test]
fn insert_and_take_round_trip() -> TestResult {
    let d = digest("b")?;
    let mut pool = WarmPool::new(WarmPoolConfig::default());
    pool.insert(container("c1", &d));
    assert_eq!(pool.count_for_digest(&d), 1);
    let taken = pool.take(&d).ok_or("warm container")?;
    assert_eq!(taken.container_id, "c1");
    assert_eq!(pool.count_for_digest(&d), 0);
    Ok(())
}

#[test]
fn per_digest_limit_evicts_oldest() -> TestResult {
    let d = digest("c")?;
    let mut pool = WarmPool::new(WarmPoolConfig {
        max_pool_size_per_digest: 2,
        max_total_warm: 16,
        ..WarmPoolConfig::default()
    });
    pool.insert(container("c1", &d));
    pool.insert(container("c2", &d));
    let evicted = pool.insert(container("c3", &d));
    assert_eq!(evicted, vec!["c1"]);
    assert_eq!(pool.count_for_digest(&d), 2);
    Ok(())
}

fn two_slot_pool() -> WarmPool {
    WarmPool::new(WarmPoolConfig {
        max_pool_size_per_digest: 3,
        max_total_warm: 2,
        ..WarmPoolConfig::default()
    })
}

#[tokio::test(start_paused = true)]
async fn total_limit_evicts_the_least_recently_inserted_across_digests() -> TestResult {
    let d1 = digest("d1")?;
    let d2 = digest("d2")?;
    let mut pool = two_slot_pool();
    pool.insert(container("a", &d1));
    tokio::time::advance(Duration::from_millis(10)).await;
    pool.insert(container("b", &d2));
    tokio::time::advance(Duration::from_millis(10)).await;
    let evicted = pool.insert(container("c", &d1));
    assert_eq!(evicted, vec!["a"]);
    assert_eq!(pool.total_count(), 2);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn touching_a_container_spares_it_from_the_next_eviction() -> TestResult {
    let d1 = digest("d1")?;
    let d2 = digest("d2")?;
    let mut pool = two_slot_pool();
    pool.insert(container("a", &d1));
    tokio::time::advance(Duration::from_millis(10)).await;
    pool.insert(container("b", &d2));
    tokio::time::advance(Duration::from_millis(10)).await;
    for held in pool.pools.values_mut().flatten() {
        if held.container_id == "a" {
            held.touch();
        }
    }
    tokio::time::advance(Duration::from_millis(10)).await;
    let evicted = pool.insert(container("c", &d1));
    assert_eq!(evicted, vec!["b"]);
    Ok(())
}

#[test]
fn population_reports_counts_per_digest() -> TestResult {
    let d = digest("e")?;
    let mut pool = WarmPool::new(WarmPoolConfig::default());
    pool.insert(container("c1", &d));
    pool.insert(container("c2", &d));
    assert_eq!(pool.population().get(&d), Some(&2));
    assert_eq!(pool.warm_digests(), vec![d]);
    assert!(pool.config().enabled);
    Ok(())
}

#[test]
fn sweeping_removes_only_containers_idle_past_the_limit() -> TestResult {
    let d = digest("f")?;
    let mut pool = WarmPool::new(WarmPoolConfig::default());
    pool.insert(container("c1", &d));
    assert!(pool.sweep_idle(Duration::from_secs(3600)).is_empty());
    assert_eq!(pool.total_count(), 1);
    assert_eq!(pool.sweep_idle(Duration::ZERO), vec!["c1"]);
    assert_eq!(pool.total_count(), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_container_reports_how_long_it_has_been_idle() -> TestResult {
    let c = container("c1", &digest("1")?);
    assert_eq!(c.idle_for(), Duration::ZERO);
    tokio::time::advance(Duration::from_secs(90)).await;
    assert_eq!(c.idle_for(), Duration::from_secs(90));
    Ok(())
}
