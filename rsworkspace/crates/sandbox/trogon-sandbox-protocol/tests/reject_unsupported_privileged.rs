//! The guard that refuses privileged specs on providers that cannot grant them.

mod provider_profile_fixture;

use serde_json::json;
use trogon_sandbox_contract::{CapabilitySupport, ProviderKind, SandboxSpec};
use trogon_sandbox_protocol::reject_unsupported_privileged;

use provider_profile_fixture::mock_profile;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn spec(privileged: bool) -> Result<SandboxSpec, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value(json!({
        "image": { "kind": "registry", "reference": { "registry": "ghcr.io", "repository": "acme/app" } },
        "workspace": { "empty": {} },
        "cpu_seconds": 10,
        "memory_mib": 512,
        "env": {},
        "privileged": privileged,
        "boot_mode": "entrypoint_default",
    }))?)
}

#[test]
fn rejects_privileged_when_the_profile_cannot_grant_it() -> TestResult {
    let error = reject_unsupported_privileged(&spec(true)?, &mock_profile(CapabilitySupport::Unsupported))
        .err()
        .ok_or("expected a rejection")?;
    assert!(error.to_string().contains("privileged"), "{error}");
    assert_eq!(error.provider(), ProviderKind::Mock);
    Ok(())
}

#[test]
fn allows_privileged_when_the_profile_grants_it() -> TestResult {
    reject_unsupported_privileged(&spec(true)?, &mock_profile(CapabilitySupport::Supported))?;
    Ok(())
}

#[test]
fn allows_unprivileged_specs_on_any_profile() -> TestResult {
    reject_unsupported_privileged(&spec(false)?, &mock_profile(CapabilitySupport::Unsupported))?;
    Ok(())
}
