use trogon_sandbox_contract::{ExtraCapability, ShmSizeMib};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn tenant(name: &str) -> Result<TenantId, trogon_sandbox_contract::TenantIdError> {
    TenantId::new(name)
}

fn shm() -> Result<ContainerOverrides, Box<dyn std::error::Error>> {
    Ok(ContainerOverrides {
        shm_size_mib: Some(ShmSizeMib::new(1024)?),
        ..Default::default()
    })
}

#[test]
fn no_override_always_passes() -> TestResult {
    let policy = HardeningOverridePolicy::deny_all();
    assert!(policy.validate(&tenant("t")?, &ContainerOverrides::default()).is_ok());
    Ok(())
}

#[test]
fn overrides_are_denied_by_default() -> TestResult {
    let result = HardeningOverridePolicy::deny_all().validate(&tenant("t")?, &shm()?);
    assert!(matches!(result, Err(LeaseError::InvalidSpec { .. })));
    Ok(())
}

#[test]
fn an_allowed_tenant_passes_and_others_still_do_not() -> TestResult {
    let policy = HardeningOverridePolicy::allowing([tenant("browser-farm")?]);
    assert!(policy.allows(&tenant("browser-farm")?));
    assert!(policy.validate(&tenant("browser-farm")?, &shm()?).is_ok());
    assert!(policy.validate(&tenant("other")?, &shm()?).is_err());
    Ok(())
}

#[test]
fn a_forbidden_capability_cannot_be_expressed() {
    assert!(ExtraCapability::new("SYS_ADMIN").is_err());
}
