use trogon_sandbox_contract::{LeaseId, TenantId};

use super::claim;
use crate::naming::{ActorName, Atespace};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn an_anonymous_claim_is_not_marked_named() -> TestResult {
    let space = Atespace::for_tenant(&TenantId::new("tenant-a")?)?;
    let lease = LeaseId::new("01ARZ");
    let actor = ActorName::for_lease(&lease)?;

    let entry = claim(&space, &actor, &lease);

    assert!(!entry.named);
    assert!(!entry.retained);
    Ok(())
}
