use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn the_heartbeat_announces_e2b_capacity_and_profile() -> TestResult {
    let beat = build_heartbeat(&WorkerId::new("w1")?, 4, 2, &[], WorkerDispatchProtocol::Legacy);
    assert_eq!(beat.provider, ProviderKind::E2b);
    assert_eq!(beat.capacity.max_concurrent_leases, 4);
    assert_eq!(beat.capacity.current_leases, 2);
    assert_eq!(beat.profile.provider, ProviderKind::E2b);
    assert_eq!(beat.dispatch_protocol, WorkerDispatchProtocol::Legacy);
    Ok(())
}

#[test]
fn the_heartbeat_advertises_configured_regions() -> TestResult {
    let id = WorkerId::new("w1")?;
    let with = build_heartbeat(&id, 4, 0, &["us".to_owned()], WorkerDispatchProtocol::TargetedV1);
    assert_eq!(with.profile.placement.regions, ["us"]);
    assert_eq!(with.dispatch_protocol, WorkerDispatchProtocol::TargetedV1);
    let without = build_heartbeat(&id, 4, 0, &[], WorkerDispatchProtocol::Legacy);
    assert!(without.profile.placement.regions.is_empty());
    Ok(())
}
