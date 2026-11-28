use std::time::{Duration, SystemTime};

use super::*;
use crate::profile::profile;
use crate::test_fixtures::TestResult;

fn digest(fill: char) -> Result<Digest, Box<dyn std::error::Error>> {
    Ok(Digest::new(format!("sha256:{}", fill.to_string().repeat(64)))?)
}

#[test]
fn the_heartbeat_reports_capacity_load_and_profile() -> TestResult {
    let worker = WorkerId::new("worker-0")?;
    let profile = profile();
    let heartbeat = build_heartbeat(&worker, 8, &profile, 3, Vec::new(), HashMap::new());
    assert_eq!(heartbeat.worker_id, worker);
    assert_eq!(heartbeat.provider, ProviderKind::K8sMicroVm);
    assert_eq!(heartbeat.capacity.max_concurrent_leases, 8);
    assert_eq!(heartbeat.capacity.current_leases, 3);
    assert_eq!(heartbeat.capacity.capabilities, WorkerCapabilities::from(&profile));
    assert_eq!(heartbeat.profile, profile);
    Ok(())
}

#[test]
fn the_heartbeat_carries_the_cache_and_the_warm_population() -> TestResult {
    let worker = WorkerId::new("worker-0")?;
    let cached = vec![digest('a')?, digest('b')?];
    let warm = HashMap::from([(digest('a')?, 2)]);
    let heartbeat = build_heartbeat(&worker, 4, &profile(), 0, cached.clone(), warm.clone());
    assert_eq!(heartbeat.cached_digests, cached);
    assert_eq!(heartbeat.warm_pool_population, warm);
    Ok(())
}

#[test]
fn the_heartbeat_is_stamped_with_the_current_time() -> TestResult {
    let worker = WorkerId::new("worker-0")?;
    let before = SystemTime::now();
    let heartbeat = build_heartbeat(&worker, 1, &profile(), 0, Vec::new(), HashMap::new());
    let age = SystemTime::now().duration_since(heartbeat.emitted_at)?;
    assert!(heartbeat.emitted_at >= before && age < Duration::from_secs(5));
    Ok(())
}
