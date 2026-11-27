use std::collections::HashMap;

use trogon_sandbox_contract::{Digest, WorkerId};

use super::build_heartbeat;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn heartbeat_includes_cached_digests_and_warm_pool() -> TestResult {
    let digest = Digest::new(format!("sha256:{}", "a".repeat(64)))?;
    let mut warm_pool_population = HashMap::new();
    warm_pool_population.insert(digest.clone(), 2);
    let worker_id = WorkerId::new("w-1")?;
    let heartbeat = build_heartbeat(
        &worker_id,
        Default::default(),
        4,
        1,
        vec![digest.clone()],
        warm_pool_population.clone(),
        None,
    );
    assert_eq!(heartbeat.cached_digests, vec![digest]);
    assert_eq!(heartbeat.warm_pool_population, warm_pool_population);
    assert_eq!(heartbeat.capacity.max_concurrent_leases, 4);
    assert_eq!(heartbeat.capacity.current_leases, 1);
    assert_eq!(heartbeat.worker_id, worker_id);
    Ok(())
}
