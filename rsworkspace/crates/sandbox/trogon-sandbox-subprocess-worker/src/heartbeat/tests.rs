use std::error::Error;

use trogon_sandbox_contract::{EgressEnforcementLevel, ProviderKind, WorkerCapabilities, WorkerId};

use super::build_heartbeat;
use crate::{IsolationBackendKind, PROFILE, profile_for_backend};

#[test]
fn the_heartbeat_carries_the_advertised_profile() -> Result<(), Box<dyn Error>> {
    let heartbeat = build_heartbeat(&WorkerId::new("w")?, 4, 1, &PROFILE);
    assert_eq!(heartbeat.provider, ProviderKind::Subprocess);
    assert_eq!(heartbeat.capacity.max_concurrent_leases, 4);
    assert_eq!(heartbeat.capacity.current_leases, 1);
    assert_eq!(heartbeat.profile, *PROFILE);
    assert_eq!(heartbeat.capacity.capabilities, WorkerCapabilities::from(&*PROFILE));
    Ok(())
}

#[test]
fn an_unisolated_backend_advertises_no_egress_enforcement() -> Result<(), Box<dyn Error>> {
    let profile = profile_for_backend(IsolationBackendKind::None);
    let heartbeat = build_heartbeat(&WorkerId::new("w")?, 4, 0, &profile);
    assert_eq!(
        heartbeat.profile.isolation.egress_enforcement,
        EgressEnforcementLevel::None
    );
    assert!(!heartbeat.profile.isolation.supports_domain_allowlist.is_supported());
    Ok(())
}
