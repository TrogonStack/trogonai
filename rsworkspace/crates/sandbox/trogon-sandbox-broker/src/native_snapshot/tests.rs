use trogon_sandbox_contract::{NativeSnapshotRef, TenantId};

use std::error::Error;

use trogon_sandbox_contract::{NativeSnapshotId, ProviderKind};

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

fn tenant(name: &str) -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new(name)?)
}

fn snap(provider: ProviderKind, id: &str) -> Result<NativeSnapshotRef, Box<dyn Error>> {
    Ok(NativeSnapshotRef {
        provider,
        id: NativeSnapshotId::new(id)?,
        expires_at: None,
    })
}

#[tokio::test]
async fn a_tenant_owns_only_what_it_recorded() -> TestResult {
    let store = InMemoryNativeSnapshotStore::new();
    let a = tenant("tenant-a")?;
    let b = tenant("tenant-b")?;
    let s = snap(ProviderKind::Daytona, "snap-1")?;
    store.record(&a, &s).await?;

    assert!(store.owns(&a, &s).await?);
    assert!(!store.owns(&b, &s).await?);
    assert!(!store.owns(&a, &snap(ProviderKind::Daytona, "snap-2")?).await?);
    Ok(())
}

#[tokio::test]
async fn the_provider_is_part_of_the_identity() -> TestResult {
    let store = InMemoryNativeSnapshotStore::new();
    let a = tenant("tenant-a")?;
    store.record(&a, &snap(ProviderKind::Daytona, "snap-1")?).await?;
    assert!(!store.owns(&a, &snap(ProviderKind::Modal, "snap-1")?).await?);
    Ok(())
}
