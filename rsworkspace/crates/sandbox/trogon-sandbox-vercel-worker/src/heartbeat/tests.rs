use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn heartbeat(plan: VercelPlan, max_vcpus: u32) -> Result<WorkerHeartbeat, Box<dyn std::error::Error + Send + Sync>> {
    let regions = vec!["iad1".to_owned(), "sfo1".to_owned()];
    Ok(build_heartbeat(
        &WorkerId::new("worker-1")?,
        4,
        1,
        &regions,
        max_vcpus,
        plan,
        WorkerDispatchProtocol::Legacy,
    ))
}

#[test]
fn the_heartbeat_reports_capacity_and_regions() -> TestResult {
    let beat = heartbeat(VercelPlan::Pro, 8)?;
    assert_eq!(beat.provider, ProviderKind::Vercel);
    assert_eq!(beat.capacity.max_concurrent_leases, 4);
    assert_eq!(beat.capacity.current_leases, 1);
    assert_eq!(beat.profile.placement.regions, ["iad1", "sfo1"]);
    assert_eq!(beat.profile.resources, resources(8));
    Ok(())
}

#[test]
fn the_plan_caps_the_session_lifetime() -> TestResult {
    let hobby = heartbeat(VercelPlan::Hobby, 4)?;
    assert_eq!(hobby.profile.lifecycle.max_lease_lifetime_seconds, Some(2700));
    let pro = heartbeat(VercelPlan::Pro, 8)?;
    assert_eq!(pro.profile.lifecycle.max_lease_lifetime_seconds, Some(86400));
    Ok(())
}

#[test]
fn the_heartbeat_announces_the_dispatch_protocol_it_is_given() -> TestResult {
    let beat = build_heartbeat(
        &WorkerId::new("worker-1")?,
        4,
        0,
        &[],
        8,
        VercelPlan::Pro,
        WorkerDispatchProtocol::TargetedV1,
    );
    assert_eq!(beat.dispatch_protocol, WorkerDispatchProtocol::TargetedV1);
    assert!(beat.supports_lease_epoch.is_supported());
    Ok(())
}
