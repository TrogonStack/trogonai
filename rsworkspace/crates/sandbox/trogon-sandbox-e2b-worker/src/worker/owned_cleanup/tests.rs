use trogon_sandbox_contract::{LeaseId, SandboxId, TenantId};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn the_request_must_arrive_on_the_subject_its_lease_names() -> TestResult {
    let request = CancelOwnedLeaseRequest {
        tenant: TenantId::new("tenant-1")?,
        lease_id: LeaseId::new("lease-1"),
        sandbox_id: SandboxId::new("sandbox-1")?,
    };
    assert_eq!(owned_subject(&request), "sandbox.provider.e2b.cancel_owned.lease-1");
    Ok(())
}
