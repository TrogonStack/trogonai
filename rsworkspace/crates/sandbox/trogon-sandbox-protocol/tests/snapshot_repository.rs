use std::collections::HashSet;

use trogon_sandbox_contract::TenantId;
use trogon_sandbox_protocol::{legacy_snapshot_repository_for_tenant, snapshot_repository_for_tenant};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn tenant(value: &str) -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::new(value)?)
}

#[test]
fn folded_tenants_get_distinct_repositories() -> TestResult {
    let tenants = ["a:b", "a/b", "a_b", "a.b", "tenant-hex", "612f62"];
    let repos: HashSet<String> = tenants
        .iter()
        .map(|t| Ok(snapshot_repository_for_tenant(&tenant(t)?)))
        .collect::<Result<_, Box<dyn std::error::Error>>>()?;
    assert_eq!(repos.len(), tenants.len());
    assert_eq!(
        legacy_snapshot_repository_for_tenant(&tenant("a:b")?),
        legacy_snapshot_repository_for_tenant(&tenant("a_b")?)
    );
    Ok(())
}

#[test]
fn tenants_without_folded_characters_keep_their_repository() -> TestResult {
    for t in ["acme", "a_b", "a.b", "org-1"] {
        assert_eq!(
            snapshot_repository_for_tenant(&tenant(t)?),
            legacy_snapshot_repository_for_tenant(&tenant(t)?)
        );
    }
    assert_eq!(
        snapshot_repository_for_tenant(&tenant("a:b")?),
        "snapshots/tenant-hex/613a62"
    );
    Ok(())
}

#[test]
fn no_tenant_owns_the_moved_repository_prefix() -> TestResult {
    let repo = snapshot_repository_for_tenant(&tenant("tenant-hex")?);
    assert_ne!(repo, "snapshots/tenant-hex");
    assert!(repo.starts_with("snapshots/tenant-hex/"));
    Ok(())
}
