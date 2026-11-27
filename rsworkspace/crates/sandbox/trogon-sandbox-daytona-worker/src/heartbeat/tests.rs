use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn the_heartbeat_reports_capacity_and_the_profile() -> TestResult {
    let beat = build_heartbeat(&WorkerId::new("worker-1")?, 4, 1);
    assert_eq!(beat.provider, ProviderKind::Daytona);
    assert_eq!(beat.worker_id, WorkerId::new("worker-1")?);
    assert_eq!(beat.capacity.max_concurrent_leases, 4);
    assert_eq!(beat.capacity.current_leases, 1);
    assert_eq!(beat.profile, *PROFILE);
    assert_eq!(beat.profile.lifecycle.max_lease_lifetime_seconds, None);
    Ok(())
}
