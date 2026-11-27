use super::*;

fn worker() -> anyhow::Result<WorkerId> {
    Ok(WorkerId::new("w1")?)
}

#[test]
fn the_heartbeat_reports_this_provider_and_its_live_capacity() -> anyhow::Result<()> {
    let heartbeat = build_heartbeat(&worker()?, 8, 3, crate::profile::for_worker(false, false), Vec::new());
    assert_eq!(heartbeat.provider, ProviderKind::Firecracker);
    assert_eq!(heartbeat.capacity.max_concurrent_leases, 8);
    assert_eq!(heartbeat.capacity.current_leases, 3);
    assert!(heartbeat.cached_digests.is_empty());
    assert!(!heartbeat.profile.isolation.supports_domain_allowlist.is_supported());
    Ok(())
}

#[test]
fn a_networked_worker_advertises_allow_lists() -> anyhow::Result<()> {
    let heartbeat = build_heartbeat(&worker()?, 8, 0, crate::profile::for_worker(true, false), Vec::new());
    assert!(heartbeat.profile.isolation.supports_domain_allowlist.is_supported());
    assert_eq!(
        heartbeat.capacity.capabilities,
        WorkerCapabilities::from(&heartbeat.profile)
    );
    Ok(())
}

#[test]
fn verb_subjects_are_scoped_to_the_firecracker_provider() {
    assert_eq!(
        verb_subject("run_command"),
        "sandbox.provider.firecracker.run_command.>"
    );
}

#[test]
fn the_lease_id_is_the_last_subject_token() -> anyhow::Result<()> {
    let id = extract_cancel_lease_id("sandbox.provider.firecracker.cancel.01J0")
        .ok_or_else(|| anyhow::anyhow!("no lease id"))?;
    assert_eq!(id.as_str(), "01J0");
    assert!(extract_cancel_lease_id("sandbox.provider.firecracker.cancel.").is_none());
    Ok(())
}
